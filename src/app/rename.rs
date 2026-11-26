//! «Групповое переименование» (Ctrl+M): masks in the style of Total Commander's
//! Multi-Rename Tool, with a live preview that shows conflicts before anything is
//! touched, per-item exclusion, safe swaps and undo (Ctrl+Z). The rules live in
//! `quokka_domain::rename`, the renaming itself in `quokka_infra::rename`.

use quokka_domain::{tr, trf};
use super::*;
use quokka_domain::rename::{self as rules, CaseMode, Planned, Rules, Source};

const ROW: f32 = 30.0;
const NAME_ID: &str = "mr-name";

/// Elements offered as chips above the preview: (inserted text, label, tooltip).
fn tokens() -> [(&'static str, &'static str, &'static str); 9] {
    [
    ("[N]", tr!("Имя", "Name"), tr!("Имя без расширения", "Name without extension")),
    ("[N1-3]", tr!("Часть имени", "Part of name"), tr!("Символы со 2-го по 5-й: [N2-5]; 3 символа со 2-го: [N2,3]; с 3-го до конца: [N3-]; последние 4: [N-4-]", "Characters 2 to 5: [N2-5]; 3 characters from the 2nd: [N2,3]; from the 3rd to the end: [N3-]; last 4: [N-4-]")),
    ("[E]", tr!("Расширение", "Extension"), tr!("Расширение без точки; диапазоны — как у имени", "Extension without the dot; ranges — same as for the name")),
    ("[C]", tr!("Счётчик", "Counter"), tr!("Начало, шаг и число цифр — в полях ниже, или прямо в маске: [C10+5:3]", "Start, step and number of digits — in the fields below, or right in the mask: [C10+5:3]")),
    ("[P]", tr!("Папка", "Folder"), tr!("Имя папки, где лежит файл; [G] — папка уровнем выше", "Name of the folder containing the file; [G] — one level higher")),
    ("[Y-M-D]", tr!("Дата", "Date"), tr!("Дата изменения: Y — год, M — месяц, D — день, в любом порядке и с любыми разделителями", "Date modified: Y — year, M — month, D — day, in any order and with any separators")),
    ("[h-m-s]", tr!("Время", "Time"), tr!("Время изменения: h — часы, m — минуты, s — секунды", "Time modified: h — hours, m — minutes, s — seconds")),
    ("[U]", tr!("ПРОПИСНЫЕ →", "UPPERCASE →"), tr!("Дальше — прописными; [L] — строчными, [F] — Каждое Слово, [n] — без изменений", "Then — uppercase; [L] — lowercase, [F] — Every Word, [n] — no change")),
    ("[S]", tr!("Размер", "Size"), tr!("Размер файла в байтах", "File size in bytes")),
]
}

fn mask_help() -> &'static str {
    tr!(
        "Элементы маски:\n\
[N] имя · [E] расширение · [P] папка · [G] папка выше\n\
[N2-5] символы 2…5 · [N2,3] 3 символа со 2-го · [N3-] с 3-го до конца · [N-4-] последние 4\n\
[C] счётчик · [C10+5:3] начать с 10, шаг 5, 3 цифры\n\
[Y] [M] [D] [h] [m] [s] дата и время изменения, напр. [Y-M-D]\n\
[S] размер в байтах\n\
[U] ПРОПИСНЫЕ · [L] строчные · [F] Каждое Слово · [n] как есть\n\
[[] и []] — сами скобки",
        "Mask elements:\n\
[N] name · [E] extension · [P] folder · [G] parent folder\n\
[N2-5] characters 2…5 · [N2,3] 3 characters from the 2nd · [N3-] from the 3rd to the end · [N-4-] last 4\n\
[C] counter · [C10+5:3] start at 10, step 5, 3 digits\n\
[Y] [M] [D] [h] [m] [s] date and time modified, e.g. [Y-M-D]\n\
[S] size in bytes\n\
[U] UPPERCASE · [L] lowercase · [F] Every Word · [n] as is\n\
[[] and []] — the brackets themselves"
    )
}

pub(super) struct RenameDialog {
    items: Vec<Source>,
    include: Vec<bool>,
    rules: Rules,
    start: String,
    step: String,
    digits: String,
    /// Plan for `planned_for`; recomputed when the rules or the selection change.
    plan: Result<Vec<Planned>, String>,
    planned_for: Option<(Rules, Vec<bool>)>,
    folder: String,
    first: bool,
    error: Option<String>,
}

impl RenameDialog {
    fn new(items: Vec<Source>, folder: String) -> Box<RenameDialog> {
        let rules = Rules::default();
        Box::new(RenameDialog {
            include: vec![true; items.len()],
            items,
            start: rules.start.to_string(),
            step: rules.step.to_string(),
            digits: rules.digits.to_string(),
            rules,
            plan: Ok(Vec::new()),
            planned_for: None,
            folder,
            first: true,
            error: None,
        })
    }

    /// Read the counter fields into the rules.
    fn counter(&mut self) -> Result<(), String> {
        self.rules.start = self.start.trim().parse().map_err(|_| tr!("Счётчик: начало — целое число", "Counter: start must be an integer").to_string())?;
        self.rules.step = self.step.trim().parse().map_err(|_| tr!("Счётчик: шаг — целое число", "Counter: step must be an integer").to_string())?;
        self.rules.digits = match self.digits.trim().parse::<usize>() {
            Ok(d @ 1..=12) => d,
            _ => return Err(tr!("Счётчик: цифр — от 1 до 12", "Counter: digits — from 1 to 12").into()),
        };
        Ok(())
    }

    fn refresh(&mut self) {
        let counter = self.counter();
        let key = (self.rules.clone(), self.include.clone());
        if self.planned_for.as_ref() == Some(&key) && counter.is_ok() {
            return;
        }
        self.plan = counter.and_then(|_| rules::plan(&self.items, &self.include, &self.rules, |p| p.symlink_metadata().is_ok()));
        self.planned_for = Some(key);
    }

    /// (to rename, problems) among the included items.
    fn counts(&self) -> (usize, usize) {
        let Ok(plan) = &self.plan else { return (0, 0) };
        let changed = plan.iter().filter(|p| p.changed).count();
        let bad = plan.iter().filter(|p| p.changed && p.problem.is_some()).count();
        (changed, bad)
    }

    fn pairs(&self) -> Vec<(PathBuf, PathBuf)> {
        let Ok(plan) = &self.plan else { return Vec::new() };
        self.items
            .iter()
            .zip(plan)
            .filter(|(_, p)| p.changed)
            .map(|(s, p)| (s.path.clone(), s.path.with_file_name(&p.new_name)))
            .collect()
    }
}

/// Insert `tok` into the field `id` at its caret (or at the end) and keep editing there.
fn insert_token(ctx: &egui::Context, id: Id, text: &mut String, tok: &str) {
    let state = egui::TextEdit::load_state(ctx, id);
    let n = text.chars().count();
    let pos = state.as_ref().and_then(|s| s.cursor.char_range()).map_or(n, |r| r.primary.index.0.min(n));
    let byte = text.char_indices().nth(pos).map_or(text.len(), |(b, _)| b);
    text.insert_str(byte, tok);
    let mut state = state.unwrap_or_default();
    let caret = egui::text::CCursor::new(pos + tok.chars().count());
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(caret)));
    state.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

fn chip(ui: &mut Ui, tok: &str, label: &str, tip: &str) -> Response {
    let g_tok = ui.painter().layout_no_wrap(tok.to_string(), mono(12.0), BLUE());
    let g_lab = ui.painter().layout_no_wrap(label.to_string(), regular(12.5), TEXT_SOFT());
    let w = g_tok.size().x + g_lab.size().x + 22.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
    let p = ui.painter();
    p.rect(r, rad(13.0), if resp.hovered() { BLUE_SOFT() } else { INPUT_BG() }, Stroke::new(1.0, if resp.hovered() { BLUE() } else { BORDER() }), eframe::egui::StrokeKind::Middle);
    let y = r.center().y;
    let x = r.left() + 9.0;
    p.galley(pos2(x, y - g_tok.size().y / 2.0), g_tok.clone(), BLUE());
    p.galley(pos2(x + g_tok.size().x + 5.0, y - g_lab.size().y / 2.0), g_lab, TEXT_SOFT());
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn small_field(ui: &mut Ui, text: &mut String, width: f32) -> Response {
    ui.add(egui::TextEdit::singleline(text).desired_width(width).margin(vec2(6.0, 5.0)).font(regular(13.5)))
}

fn label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(TEXT_SOFT()));
}

impl QuokkaApp {
    pub(super) fn open_multi_rename(&mut self) {
        let p = self.ap_ref();
        let pick = |e: &&quokka_domain::Entry| !e.is_parent && !e.is_drive;
        let entries: Vec<&quokka_domain::Entry> = if p.marked.is_empty() {
            p.current().into_iter().filter(pick).collect()
        } else {
            // in the order of the list: the counter follows it
            p.all.iter().filter(|e| p.marked.contains(&e.path)).filter(pick).collect()
        };
        if entries.is_empty() {
            self.toast(tr!("Отметьте файлы и папки для переименования", "Mark files and folders to rename"), false);
            return;
        }
        let items = entries
            .iter()
            .map(|e| Source { path: e.path.clone(), is_dir: e.is_dir, size: e.size, modified: e.modified })
            .collect();
        let folder = if p.results.is_some() { p.results_label.clone() } else { p.title() };
        self.modal = Modal::MultiRename(RenameDialog::new(items, folder));
    }

    /// Returns whether the dialog stays open.
    pub(super) fn multi_rename_ui(&mut self, ctx: &egui::Context, d: &mut RenameDialog, esc: bool, enter: bool) -> bool {
        if esc {
            return false;
        }
        d.refresh();
        let (changed, bad) = d.counts();
        let can_apply = d.plan.is_ok() && changed > 0 && bad == 0;
        let mut apply = enter && can_apply;
        let mut cancel = false;
        let mut reset = false;
        let history = self.settings.rename_masks.clone();
        let screen = ctx.content_rect();
        let width = (screen.width() - 80.0).clamp(560.0, 940.0);
        let list_h = (screen.height() - 470.0).clamp(140.0, 420.0);
        let name_id = Id::unique(NAME_ID);

        dialog(ctx, "multi-rename", width, |ui| {
            let sub = format!("{} · {}", items_text(d.items.len() as u64), d.folder);
            dialog_header(ui, Glyph::Pencil, BLUE(), tr!("Групповое переименование", "Multi-rename"), Some(&sub));
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            let w = ui.available_width();

            // ── masks
            ui.horizontal(|ui| {
                let ext_w = 190.0;
                ui.vertical(|ui| {
                    ui.set_width(w - ext_w - 12.0);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    label(ui, tr!("Маска имени", "Name mask"));
                    let (out, _) = jb_field(ui, name_id, &mut d.rules.name_mask, "[N]", Glyph::Pencil, mono(14.0), &history, |ui| {
                        jb_tool(ui, Glyph::Question, true, mask_help());
                    });
                    if d.first {
                        d.first = false;
                        out.response.request_focus();
                        let mut st = out.state;
                        let n = d.rules.name_mask.chars().count();
                        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
                        st.store(ui.ctx(), name_id);
                    }
                });
                ui.vertical(|ui| {
                    ui.set_width(ext_w);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    label(ui, tr!("Расширение", "Extension"));
                    jb_field(ui, Id::unique("mr-ext"), &mut d.rules.ext_mask, tr!("без расширения", "no extension"), Glyph::Document, mono(14.0), &[], |_| {});
                });
            });

            // ── elements to insert
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (tok, name, tip) in tokens() {
                    if chip(ui, tok, name, tip).clicked() {
                        insert_token(ui.ctx(), name_id, &mut d.rules.name_mask, tok);
                    }
                }
            });
            ui.add_space(2.0);

            // ── search and replace
            ui.horizontal(|ui| {
                let half = (w - 12.0) / 2.0;
                ui.vertical(|ui| {
                    ui.set_width(half);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    label(ui, tr!("Найти", "Find"));
                    let (regex, case) = (d.rules.regex, d.rules.match_case);
                    let mut toggle = (false, false);
                    let hint = if regex { tr!("регулярное выражение", "regular expression") } else { tr!("текст; несколько — через |", "text; several — separated by |") };
                    jb_field(ui, Id::unique("mr-find"), &mut d.rules.find, hint, Glyph::Search, mono(13.5), &[], |ui| {
                        toggle.0 = field_toggle(ui, ".*", regex, tr!("Регулярное выражение; в замене $1, $2 — найденные группы", "Regular expression; in the replacement $1, $2 — captured groups")).clicked();
                        toggle.1 = field_toggle(ui, "Aa", case, tr!("Учитывать регистр букв", "Match letter case")).clicked();
                    });
                    if toggle.0 {
                        d.rules.regex = !regex;
                    }
                    if toggle.1 {
                        d.rules.match_case = !case;
                    }
                });
                ui.vertical(|ui| {
                    ui.set_width(half);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    label(ui, tr!("Заменить на", "Replace with"));
                    let hint = if d.rules.regex { tr!("$1 — первая группа", "$1 — first group") } else { tr!("замены через | в том же порядке", "replacements separated by | in the same order") };
                    jb_field(ui, Id::unique("mr-replace"), &mut d.rules.replace, hint, Glyph::ArrowRight, mono(13.5), &[], |_| {});
                });
            });

            // ── case and counter
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                label(ui, tr!("Регистр", "Case"));
                let cur = CaseMode::all().iter().find(|c| c.0 == d.rules.case).map_or("", |c| c.1);
                egui::ComboBox::from_id_salt("mr-case").selected_text(cur).width(210.0).show_ui(ui, |ui| {
                    for (m, name) in CaseMode::all() {
                        ui.selectable_value(&mut d.rules.case, m, name);
                    }
                });
                ui.add_space(18.0);
                label(ui, tr!("Счётчик [C]: начать с", "Counter [C]: start at"));
                small_field(ui, &mut d.start, 54.0);
                label(ui, tr!("шаг", "step"));
                small_field(ui, &mut d.step, 40.0);
                label(ui, tr!("цифр", "digits"));
                small_field(ui, &mut d.digits, 32.0).on_hover_text(tr!("Ведущие нули: 3 → 001, 002…", "Leading zeros: 3 → 001, 002…"));
            });

            // ── preview
            ui.add_space(4.0);
            let (head, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(head, CornerRadius { nw: rad_u8(6), ne: rad_u8(6), ..Default::default() }, HEADER_BG());
            let col_old = head.left() + 36.0;
            let col_new = head.left() + 36.0 + (w - 36.0) * 0.42;
            let all_on = d.include.iter().all(|&v| v);
            let toggle_all = ui.interact(Rect::from_center_size(pos2(head.left() + 16.0, head.center().y), vec2(20.0, 20.0)), Id::unique("mr-all"), Sense::click());
            draw_check(ui.painter(), toggle_all.rect, all_on, d.include.iter().any(|&v| v));
            if toggle_all.on_hover_text(tr!("Отметить все / снять", "Mark all / unmark")).clicked() {
                d.include.iter_mut().for_each(|v| *v = !all_on);
            }
            let p = ui.painter();
            p.text(pos2(col_old, head.center().y), Align2::LEFT_CENTER, tr!("Было", "Before"), semibold(12.5), TEXT_SOFT());
            p.text(pos2(col_new, head.center().y), Align2::LEFT_CENTER, tr!("Станет", "After"), semibold(12.5), TEXT_SOFT());

            egui::Frame::NONE.stroke(Stroke::new(1.0, BORDER())).show(ui, |ui| {
                ui.set_width(w - 2.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                match &d.plan {
                    Err(e) => {
                        let (r, _) = ui.allocate_exact_size(vec2(w - 2.0, list_h), Sense::hover());
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, e, regular(13.5), RED);
                    }
                    Ok(plan) => {
                        let mut flip = None;
                        egui::ScrollArea::vertical().id_salt("mr-list").max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show_rows(
                            ui,
                            ROW,
                            plan.len(),
                            |ui, range| {
                                for i in range {
                                    let (src, pl) = (&d.items[i], &plan[i]);
                                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                                    let p = ui.painter();
                                    if resp.hovered() {
                                        p.rect_filled(r, 0.0, HOVER());
                                    }
                                    p.hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, ROW_LINE()));
                                    let on = d.include[i];
                                    draw_check(p, Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(20.0, 20.0)), on, on);
                                    let g = if src.is_dir { Glyph::Folder } else { Glyph::Document };
                                    let gr = Rect::from_center_size(pos2(col_old + 7.0, r.center().y), vec2(14.0, 14.0));
                                    icons::glyph(p, gr, g, if src.is_dir { FOLDER_BACK } else { SLATE() });
                                    let old = src.name();
                                    let old_clip = p.with_clip_rect(Rect::from_min_max(pos2(col_old, r.top()), pos2(col_new - 30.0, r.bottom())));
                                    old_clip.text(pos2(col_old + 22.0, r.center().y), Align2::LEFT_CENTER, &old, regular(13.5), if on { TEXT() } else { MUTED() });
                                    let (fg, font) = match (on, pl.changed, pl.problem) {
                                        (false, _, _) => (MUTED(), regular(13.5)),
                                        (_, true, Some(_)) => (RED, semibold(13.5)),
                                        (_, true, None) => (BLUE(), semibold(13.5)),
                                        _ => (MUTED(), regular(13.5)),
                                    };
                                    if pl.changed {
                                        icons::glyph(p, Rect::from_center_size(pos2(col_new - 15.0, r.center().y), vec2(12.0, 12.0)), Glyph::ArrowRight, fg);
                                    }
                                    let new_text = if on { pl.new_name.as_str() } else { tr!("не переименовывается", "not renamed") };
                                    let new_clip = p.with_clip_rect(Rect::from_min_max(pos2(col_new, r.top()), pos2(r.right() - 8.0, r.bottom())));
                                    let end = new_clip.text(pos2(col_new, r.center().y), Align2::LEFT_CENTER, new_text, font, fg).right();
                                    let mut resp = resp;
                                    if let (true, Some(problem)) = (pl.changed, pl.problem) {
                                        let msg = format!("— {problem}");
                                        new_clip.text(pos2(end + 10.0, r.center().y), Align2::LEFT_CENTER, &msg, regular(12.5), RED);
                                        resp = resp.on_hover_text(problem.to_string());
                                    }
                                    if resp.clicked() {
                                        flip = Some(i);
                                    }
                                }
                            },
                        );
                        if let Some(i) = flip {
                            d.include[i] = !d.include[i];
                        }
                    }
                }
            });

            // ── status and buttons
            ui.add_space(4.0);
            if let Some(e) = &d.error {
                ui.label(RichText::new(e.as_str()).color(RED).size(13.0));
            }
            ui.horizontal(|ui| {
                let total = d.include.iter().filter(|&&v| v).count();
                let (text, col) = match (&d.plan, changed, bad) {
                    (Err(_), _, _) => (tr!("Исправьте маску", "Fix the mask").to_string(), RED),
                    (_, _, b) if b > 0 => (trf!("Конфликтов: {b} — исправьте маску или снимите галочки", "Conflicts: {b} — fix the mask or uncheck items"), RED),
                    (_, 0, _) => (tr!("Имена не меняются", "Names do not change").to_string(), MUTED()),
                    (_, c, _) => (trf!("Будет переименовано: {c} из {total}", "Will be renamed: {c} of {total}"), TEXT_SOFT()),
                };
                ui.label(RichText::new(text).size(13.0).color(col));
                if ui.link(RichText::new(tr!("Сбросить", "Reset")).size(13.0).color(BLUE())).clicked() {
                    reset = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(can_apply, |ui| {
                        if btn(ui, tr!("Переименовать", "Rename"), BLUE(), Color32::WHITE).on_hover_text(tr!("Enter; отменить потом — Ctrl+Z", "Enter; undo later — Ctrl+Z")).clicked() {
                            apply = true;
                        }
                    });
                    if btn(ui, tr!("Отмена", "Cancel"), INPUT_BG(), TEXT()).clicked() {
                        cancel = true;
                    }
                });
            });
        });

        if reset {
            let include = std::mem::take(&mut d.include);
            let fresh = RenameDialog::new(std::mem::take(&mut d.items), std::mem::take(&mut d.folder));
            *d = *fresh;
            d.include = include.iter().map(|_| true).collect();
            return true;
        }
        if cancel {
            return false;
        }
        if !apply || !can_apply {
            return true;
        }
        let pairs = d.pairs();
        match quokka_infra::rename::rename_batch(&pairs) {
            Ok(done) => {
                config::remember(&mut self.settings.rename_masks, &d.rules.name_mask);
                self.save_settings();
                let dirs: Vec<PathBuf> = done.iter().filter_map(|(_, p)| p.parent().map(PathBuf::from)).collect();
                self.reload_dirs(&dirs);
                // keep the renamed items marked, so the next step can work on them
                let p = self.ap();
                p.marked = if p.marked.is_empty() { Default::default() } else { done.iter().map(|(_, n)| n.clone()).collect() };
                if let [(_, new)] = done.as_slice() {
                    if let Some(n) = new.file_name() {
                        p.select_name(&n.to_string_lossy());
                    }
                }
                let text = trf!("Переименовано: {}", "Renamed: {}", items_text(done.len() as u64));
                self.undo = Some(Undo::Rename(done));
                self.toast = Some(Toast::undo(text));
                false
            }
            Err(e) => {
                d.error = Some(e);
                d.planned_for = None;
                true
            }
        }
    }
}

/// Checkbox look without a label; `partial` draws a dash when not all are on.
fn draw_check(p: &egui::Painter, r: Rect, on: bool, partial: bool) {
    let b = Rect::from_center_size(r.center(), vec2(15.0, 15.0));
    if on {
        p.rect_filled(b, rad(4.0), BLUE());
        let c = b.center();
        p.line_segment([c + vec2(-3.8, 0.2), c + vec2(-1.0, 3.0)], Stroke::new(1.8, Color32::WHITE));
        p.line_segment([c + vec2(-1.0, 3.0), c + vec2(4.0, -2.8)], Stroke::new(1.8, Color32::WHITE));
    } else {
        p.rect(b, rad(4.0), CARD(), Stroke::new(1.2, FIELD_BORDER()), eframe::egui::StrokeKind::Middle);
        if partial {
            p.hline((b.left() + 4.0)..=(b.right() - 4.0), b.center().y, Stroke::new(1.8, BLUE()));
        }
    }
}
