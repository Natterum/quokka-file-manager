//! «Поиск файлов» (Alt+F7) — dialog in the spirit of Total Commander's Find Files.
//! The search itself runs in `crate::search`; this module holds the form, the live
//! result list and what can be done with a result (go to it, view, edit, feed to panel).

use super::*;
use crate::config;
use crate::search::{Hit, Kinds, Query, Search, SizeCmp, TextQuery};
use std::path::Path;
use std::time::{Duration, SystemTime};

const ROW: f32 = 44.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FindTab {
    Main,
    More,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DateMode {
    Any,
    Within,
    Between,
}

const DEPTHS: [(Option<usize>, &str); 7] = [
    (None, "Во всех вложенных папках"),
    (Some(0), "Только в этой папке"),
    (Some(1), "На 1 уровень вглубь"),
    (Some(2), "На 2 уровня вглубь"),
    (Some(3), "На 3 уровня вглубь"),
    (Some(5), "На 5 уровней вглубь"),
    (Some(10), "На 10 уровней вглубь"),
];
const SIZE_CMPS: [(SizeCmp, &str); 3] = [(SizeCmp::Less, "меньше"), (SizeCmp::Equal, "равен"), (SizeCmp::Greater, "больше")];
const SIZE_UNITS: [(u64, &str); 4] = [(1, "Б"), (1024, "КБ"), (1 << 20, "МБ"), (1 << 30, "ГБ")];
const AGE_UNITS: [(u64, &str); 6] = [
    (60, "минут"),
    (3600, "часов"),
    (86_400, "дней"),
    (604_800, "недель"),
    (2_592_000, "месяцев"),
    (31_536_000, "лет"),
];

pub(super) struct FindDialog {
    roots: String,
    mask: String,
    mask_regex: bool,
    mask_case: bool,
    depth: Option<usize>,
    kinds: Kinds,
    hidden: bool,
    archives: bool,
    text_on: bool,
    text: String,
    text_case: bool,
    text_words: bool,
    text_regex: bool,
    text_not: bool,
    text_enc: bool,
    size_on: bool,
    size_cmp: SizeCmp,
    size_val: String,
    size_unit: usize,
    date_mode: DateMode,
    age_val: String,
    age_unit: usize,
    date_from: String,
    date_to: String,
    tab: FindTab,
    search: Option<Search>,
    /// Folder results are shown relative to when fed to a panel.
    search_root: PathBuf,
    search_label: String,
    sel: usize,
    error: Option<String>,
    focus_mask: bool,
    scroll_to_sel: bool,
    list_off: f32,
    list_h: f32,
}

/// What the user asked for this frame.
enum Act {
    Start,
    Stop,
    Close,
    Clear,
    GoTo,
    View,
    Edit,
    Open,
    CopyPath,
    Feed,
}

fn parse_date(s: &str, end_of_day: bool) -> Result<Option<SystemTime>, String> {
    use chrono::TimeZone;
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let d = chrono::NaiveDate::parse_from_str(s, "%d.%m.%Y").map_err(|_| format!("Дата «{s}» должна быть в виде ДД.ММ.ГГГГ"))?;
    let t = if end_of_day { d.and_hms_opt(23, 59, 59) } else { d.and_hms_opt(0, 0, 0) };
    Ok(t.and_then(|t| chrono::Local.from_local_datetime(&t).earliest()).map(SystemTime::from))
}

impl FindDialog {
    pub(super) fn new(hidden: bool) -> Box<FindDialog> {
        Box::new(FindDialog {
            roots: String::new(),
            mask: String::new(),
            mask_regex: false,
            mask_case: false,
            depth: None,
            kinds: Kinds::All,
            hidden,
            archives: false,
            text_on: false,
            text: String::new(),
            text_case: false,
            text_words: false,
            text_regex: false,
            text_not: false,
            text_enc: false,
            size_on: false,
            size_cmp: SizeCmp::Greater,
            size_val: String::new(),
            size_unit: 2,
            date_mode: DateMode::Any,
            age_val: "7".into(),
            age_unit: 2,
            date_from: String::new(),
            date_to: String::new(),
            tab: FindTab::Main,
            search: None,
            search_root: PathBuf::new(),
            search_label: String::new(),
            sel: 0,
            error: None,
            focus_mask: true,
            scroll_to_sel: false,
            list_off: 0.0,
            list_h: 300.0,
        })
    }

    fn advanced_count(&self) -> usize {
        self.size_on as usize + (self.date_mode != DateMode::Any) as usize
    }

    fn roots(&self, base: &Path) -> Vec<PathBuf> {
        self.roots
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                let p = PathBuf::from(s);
                if p.is_relative() && !base.as_os_str().is_empty() {
                    base.join(p)
                } else {
                    p
                }
            })
            .collect()
    }

    fn query(&self, base: &Path) -> Result<Query, String> {
        let text = (self.text_on && !self.text.is_empty()).then(|| TextQuery {
            text: self.text.clone(),
            case: self.text_case,
            words: self.text_words,
            regex: self.text_regex,
            not_containing: self.text_not,
            other_encodings: self.text_enc,
        });
        let size = if self.size_on {
            let v: u64 = self.size_val.trim().parse().map_err(|_| "Размер — целое неотрицательное число".to_string())?;
            Some((self.size_cmp, v, SIZE_UNITS[self.size_unit].0))
        } else {
            None
        };
        let (mut newer_than, mut older_than) = (None, None);
        match self.date_mode {
            DateMode::Any => {}
            DateMode::Within => {
                let n: u64 = self.age_val.trim().parse().map_err(|_| "Возраст файла — целое число".to_string())?;
                newer_than = SystemTime::now().checked_sub(Duration::from_secs(n.saturating_mul(AGE_UNITS[self.age_unit].0)));
            }
            DateMode::Between => {
                newer_than = parse_date(&self.date_from, false)?;
                older_than = parse_date(&self.date_to, true)?;
                if newer_than.is_none() && older_than.is_none() {
                    return Err("Укажите хотя бы одну дату".into());
                }
            }
        }
        Ok(Query {
            roots: self.roots(base),
            mask: self.mask.clone(),
            mask_regex: self.mask_regex,
            mask_case: self.mask_case,
            depth: self.depth,
            kinds: self.kinds,
            hidden: self.hidden,
            archives: self.archives,
            text,
            size,
            newer_than,
            older_than,
        })
    }

    fn hit(&self) -> Option<Hit> {
        self.search.as_ref().and_then(|s| s.hits.get(self.sel)).cloned()
    }

    fn stop(&mut self) {
        if let Some(s) = &mut self.search {
            s.stop();
        }
    }
}

/// Small ▾ button listing earlier values of a field.
fn history(ui: &mut Ui, id: &str, items: &[String], target: &mut String) {
    ui.add_enabled_ui(!items.is_empty(), |ui| {
        ui.push_id(id, |ui| {
            ui.menu_button(RichText::new("▾").size(15.0).color(TEXT_SOFT), |ui| {
                menu_style(ui, 320.0);
                menu_title(ui, "Недавние");
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for h in items {
                        if menu_item(ui, h, "", true, false).clicked() {
                            *target = h.clone();
                            ui.close_menu();
                        }
                    }
                });
            })
            .response
            .on_hover_text("История")
        });
    });
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.add_sized(vec2(118.0, 30.0), egui::Label::new(RichText::new(text).color(TEXT_SOFT).size(13.5)).wrap());
}

/// Keep wrapped controls and help text aligned with their field on every line.
fn field_body(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        ui.allocate_exact_size(vec2(118.0, 0.0), Sense::hover());
        ui.vertical(add);
    });
}

fn find_section(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::none()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(10.0)
        .inner_margin(Margin::same(12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).font(semibold(13.0)).color(TEXT));
            add(ui);
        });
}

impl QuokkaApp {
    /// Alt+F7: reopen the last search (with its results) or start a new one here.
    pub(super) fn open_find(&mut self) {
        let p = self.ap_ref();
        let marked_dirs: Vec<String> = p
            .all
            .iter()
            .filter(|e| e.is_dir && !e.is_parent && p.marked.contains(&e.path))
            .map(|e| e.path.display().to_string())
            .collect();
        let here = if !marked_dirs.is_empty() {
            marked_dirs.join("; ")
        } else if p.is_pc() {
            self.drives.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join("; ")
        } else {
            p.path.display().to_string()
        };
        let mut fd = self.find.take().unwrap_or_else(|| FindDialog::new(self.settings.show_hidden));
        if fd.search.as_ref().map_or(true, |s| s.hits.is_empty() && !s.running) {
            fd.roots = here;
            fd.search = None;
        }
        fd.focus_mask = fd.search.is_none();
        fd.scroll_to_sel = true;
        self.modal = Modal::Find(fd);
    }

    /// Draw the dialog; returns whether it stays open.
    pub(super) fn find_ui(&mut self, ctx: &egui::Context, fd: &mut FindDialog, esc: bool, enter: bool) -> bool {
        if let Some(s) = &mut fd.search {
            s.poll();
            if s.running {
                ctx.request_repaint_after(Duration::from_millis(120));
            }
        }
        let running = fd.search.as_ref().is_some_and(|s| s.running);
        let n = fd.search.as_ref().map_or(0, |s| s.hits.len());
        fd.sel = fd.sel.min(n.saturating_sub(1));
        let typing = ctx.wants_keyboard_input();
        let mut act = None;

        if esc {
            act = Some(if running { Act::Stop } else { Act::Close });
        } else if !typing {
            let page = ((fd.list_h / ROW) as isize - 1).max(1);
            ctx.input_mut(|i| {
                let mut moved = None;
                for (k, d) in [(Key::ArrowUp, -1), (Key::ArrowDown, 1), (Key::PageUp, -page), (Key::PageDown, page)] {
                    if i.consume_key(Modifiers::NONE, k) {
                        moved = Some(fd.sel as isize + d);
                    }
                }
                if i.consume_key(Modifiers::NONE, Key::Home) {
                    moved = Some(0);
                }
                if i.consume_key(Modifiers::NONE, Key::End) {
                    moved = Some(n as isize - 1);
                }
                if let Some(m) = moved {
                    if n > 0 {
                        fd.sel = m.clamp(0, n as isize - 1) as usize;
                        fd.scroll_to_sel = true;
                    }
                }
                if n > 0 {
                    if i.consume_key(Modifiers::NONE, Key::F3) {
                        act = Some(Act::View);
                    } else if i.consume_key(Modifiers::NONE, Key::F4) {
                        act = Some(Act::Edit);
                    } else if i.consume_key(Modifiers::NONE, Key::Enter) {
                        act = Some(Act::GoTo);
                    }
                } else if !running && i.consume_key(Modifiers::NONE, Key::Enter) {
                    act = Some(Act::Start);
                }
            });
        }

        let screen = ctx.screen_rect();
        let width = (screen.width() - 88.0).min(940.0);
        let list_h = (screen.height() - 750.0).clamp(100.0, 300.0);
        let masks = self.settings.find_masks.clone();
        let texts = self.settings.find_texts.clone();

        dialog(ctx, "find", width, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
            dialog_header(ui, Glyph::Search, BLUE, "Поиск файлов", Some("По имени, содержимому и свойствам · Alt+F7"));
            ui.horizontal(|ui| {
                if pill(ui, "Основные", fd.tab == FindTab::Main).clicked() {
                    fd.tab = FindTab::Main;
                }
                let k = fd.advanced_count();
                let more = if k > 0 {
                    format!("Дополнительно · {k}")
                } else {
                    "Дополнительно".to_string()
                };
                if pill(ui, &more, fd.tab == FindTab::More).clicked() {
                    fd.tab = FindTab::More;
                }
            });
            ui.add_space(2.0);

            egui::ScrollArea::vertical().id_salt("find-form").max_height((screen.height() - 400.0).clamp(180.0, 420.0)).auto_shrink([false, true]).show(ui, |ui| {
            ui.add_enabled_ui(!running, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.spacing_mut().button_padding = vec2(BUTTON_PADDING_X, BUTTON_PADDING_Y);
                match fd.tab {
                    FindTab::Main => {
                        find_section(ui, "Имя и расположение", |ui| {
                        let wide = ui.available_width() - 126.0;
                        ui.horizontal(|ui| {
                            field_label(ui, "Где искать");
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut fd.roots)
                                    .desired_width((wide - 20.0).max(80.0))
                                    .margin(vec2(10.0, 6.0))
                                    .font(regular(14.0))
                                    .hint_text("Папка; несколько папок — через «;»"),
                            );
                            if r.lost_focus() && enter {
                                act = Some(Act::Start);
                            }
                        });
                        ui.horizontal(|ui| {
                            field_label(ui, "Имя файла");
                            let hint = if fd.mask_regex { r"^IMG_\d{4}\.jpe?g$" } else { "*.txt;*.doc | temp*  — пусто: все файлы" };
                            let mut out = egui::TextEdit::singleline(&mut fd.mask)
                                .desired_width((wide - 180.0).max(80.0))
                                .margin(vec2(10.0, 6.0))
                                .font(if fd.mask_regex { mono(14.0) } else { regular(14.0) })
                                .hint_text(RichText::new(hint).color(MUTED))
                                .show(ui);
                            if fd.focus_mask {
                                fd.focus_mask = false;
                                out.response.request_focus();
                                let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(fd.mask.chars().count()));
                                out.state.cursor.set_char_range(Some(all));
                                out.state.store(ui.ctx(), out.response.id);
                            }
                            if out.response.lost_focus() && enter {
                                act = Some(Act::Start);
                            }
                            history(ui, "mask-hist", &masks, &mut fd.mask);
                            if pill(ui, ".*", fd.mask_regex).on_hover_text("Регулярное выражение вместо масок").clicked() {
                                fd.mask_regex = !fd.mask_regex;
                            }
                            if pill(ui, "Aa", fd.mask_case).on_hover_text("Учитывать регистр в имени").clicked() {
                                fd.mask_case = !fd.mask_case;
                            }
                        });
                        field_body(ui, |ui| {
                            let tip = if fd.mask_regex {
                                "Регулярное выражение ищется в имени файла (без пути)."
                            } else {
                                "Маски через «;» или пробел. Без * и ? ищется часть имени. После «|» — исключения: файлы (*.bak) и папки (node_modules\\)."
                            };
                            ui.add(egui::Label::new(RichText::new(tip).size(12.0).color(MUTED)).wrap());
                        });
                        });
                        find_section(ui, "Содержимое файла", |ui| {
                        let wide = ui.available_width() - 126.0;
                        ui.horizontal(|ui| {
                            ui.add_sized(vec2(118.0, 30.0), egui::Checkbox::new(&mut fd.text_on, RichText::new("С текстом").color(TEXT_SOFT).size(13.5)));
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut fd.text)
                                    .desired_width((wide - 68.0).max(80.0))
                                    .margin(vec2(10.0, 6.0))
                                    .font(if fd.text_regex { mono(14.0) } else { regular(14.0) })
                                    .hint_text(RichText::new("Текст внутри файла").color(MUTED)),
                            );
                            if r.changed() {
                                fd.text_on = !fd.text.is_empty();
                            }
                            if r.lost_focus() && enter {
                                act = Some(Act::Start);
                            }
                            let before = fd.text.clone();
                            history(ui, "text-hist", &texts, &mut fd.text);
                            if fd.text != before {
                                fd.text_on = true;
                            }
                        });
                        field_body(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (on, label, tip) in [
                                (&mut fd.text_case, "Aa  Регистр", "Различать заглавные и строчные буквы"),
                                (&mut fd.text_words, "Слова целиком", "«кот» не найдётся в «который»"),
                                (&mut fd.text_regex, ".*  Регулярное выражение", "Текст — регулярное выражение; ^ и $ — начало и конец строки"),
                                (&mut fd.text_not, "Не содержит", "Найти файлы, в которых этого текста НЕТ"),
                                (&mut fd.text_enc, "UTF-16 и Windows-1251", "Искать текст ещё и в кодировках UTF-16 LE и Windows-1251 (не для регулярных выражений)"),
                            ] {
                                if pill(ui, label, *on).on_hover_text(tip).clicked() {
                                    *on = !*on;
                                    fd.text_on = true;
                                }
                            }
                        });
                        });
                        });
                        find_section(ui, "Область поиска", |ui| {
                        ui.horizontal_top(|ui| {
                            field_label(ui, "Глубина");
                            ui.horizontal_wrapped(|ui| {
                            let cur = DEPTHS.iter().find(|(d, _)| *d == fd.depth).map_or("", |(_, l)| l);
                            egui::ComboBox::from_id_salt("find-depth").selected_text(cur).width(240.0).show_ui(ui, |ui| {
                                for (d, l) in DEPTHS {
                                    ui.selectable_value(&mut fd.depth, d, l);
                                }
                            });
                            for (k, l) in [(Kinds::All, "Файлы и папки"), (Kinds::Files, "Только файлы"), (Kinds::Dirs, "Только папки")] {
                                if pill(ui, l, fd.kinds == k).clicked() {
                                    fd.kinds = k;
                                }
                            }
                        });
                        });
                        field_body(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut fd.hidden, RichText::new("Скрытые и системные").color(TEXT));
                            ui.checkbox(&mut fd.archives, RichText::new("Внутри ZIP-архивов").color(TEXT))
                                .on_hover_text("Искать среди имён файлов в ZIP-архивах (текст внутри архивов не ищется)");
                        });
                        });
                        });
                    }
                    FindTab::More => {
                        find_section(ui, "Размер файла", |ui| {
                        ui.horizontal(|ui| {
                            ui.add_sized(vec2(118.0, 30.0), egui::Checkbox::new(&mut fd.size_on, RichText::new("Размер").color(TEXT_SOFT).size(13.5)));
                            ui.add_enabled_ui(fd.size_on, |ui| {
                                let cur = SIZE_CMPS.iter().find(|(c, _)| *c == fd.size_cmp).map_or("", |(_, l)| l);
                                egui::ComboBox::from_id_salt("find-size-cmp").selected_text(cur).width(110.0).show_ui(ui, |ui| {
                                    for (c, l) in SIZE_CMPS {
                                        ui.selectable_value(&mut fd.size_cmp, c, l);
                                    }
                                });
                                ui.add(egui::TextEdit::singleline(&mut fd.size_val).desired_width(90.0).margin(vec2(8.0, 6.0)).hint_text("100"));
                                egui::ComboBox::from_id_salt("find-size-unit").selected_text(SIZE_UNITS[fd.size_unit].1).width(70.0).show_ui(ui, |ui| {
                                    for (i, (_, l)) in SIZE_UNITS.iter().enumerate() {
                                        ui.selectable_value(&mut fd.size_unit, i, *l);
                                    }
                                });
                            });
                            if fd.size_on && fd.size_val.trim().is_empty() {
                                fd.size_val = "100".into();
                            }
                        });
                        field_body(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new("Папки при фильтре по размеру не показываются. «Равен» сравнивает с округлением до выбранной единицы.").size(12.0).color(MUTED)).wrap());
                        });
                        });
                        find_section(ui, "Дата изменения", |ui| {
                        ui.horizontal(|ui| {
                            field_label(ui, "Дата изменения");
                            ui.radio_value(&mut fd.date_mode, DateMode::Any, RichText::new("Любая").color(TEXT));
                        });
                        field_body(ui, |ui| { ui.horizontal(|ui| {
                            ui.radio_value(&mut fd.date_mode, DateMode::Within, RichText::new("Не старше").color(TEXT));
                            ui.add_enabled_ui(fd.date_mode == DateMode::Within, |ui| {
                                ui.add(egui::TextEdit::singleline(&mut fd.age_val).desired_width(60.0).margin(vec2(8.0, 6.0)));
                                egui::ComboBox::from_id_salt("find-age-unit").selected_text(AGE_UNITS[fd.age_unit].1).width(100.0).show_ui(ui, |ui| {
                                    for (i, (_, l)) in AGE_UNITS.iter().enumerate() {
                                        ui.selectable_value(&mut fd.age_unit, i, *l);
                                    }
                                });
                            });
                        }); });
                        field_body(ui, |ui| { ui.horizontal(|ui| {
                            ui.radio_value(&mut fd.date_mode, DateMode::Between, RichText::new("В промежутке с").color(TEXT));
                            ui.add_enabled_ui(fd.date_mode == DateMode::Between, |ui| {
                                ui.add(egui::TextEdit::singleline(&mut fd.date_from).desired_width(110.0).margin(vec2(8.0, 6.0)).hint_text("ДД.ММ.ГГГГ"));
                                ui.label(RichText::new("по").color(TEXT));
                                ui.add(egui::TextEdit::singleline(&mut fd.date_to).desired_width(110.0).margin(vec2(8.0, 6.0)).hint_text("ДД.ММ.ГГГГ"));
                            });
                        }); });
                        field_body(ui, |ui| {
                            ui.label(RichText::new("Любую из двух дат можно не заполнять.").size(12.0).color(MUTED));
                        });
                        });
                    }
                }
            });
            });

            if let Some(e) = &fd.error {
                ui.label(RichText::new(e.as_str()).color(RED).size(13.0));
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Результаты").font(semibold(14.0)).color(TEXT));
                if fd.search.is_some() {
                    ui.label(RichText::new(n.to_string()).size(13.0).color(BLUE));
                }
            });

            // status line
            if let Some(s) = &fd.search {
                ui.horizontal(|ui| {
                    if s.running {
                        ui.add(egui::Spinner::new().size(14.0).color(BLUE));
                    }
                    let mut status = if s.running {
                        format!("Найдено: {n} · просмотрено файлов: {}, папок: {}", s.files, s.dirs)
                    } else {
                        format!(
                            "{} за {:.1} с · найдено: {n} · просмотрено файлов: {}, папок: {}",
                            if s.cancelled { "Остановлено" } else { "Готово" },
                            s.elapsed.as_secs_f32(),
                            s.files,
                            s.dirs
                        )
                    };
                    if s.errors > 0 {
                        status.push_str(&format!(" · недоступно: {}", s.errors));
                    }
                    ui.label(RichText::new(status).size(13.0).color(TEXT_SOFT));
                    if s.running {
                        ui.add(egui::Label::new(RichText::new(s.current.as_str()).size(12.0).color(MUTED)).truncate());
                    }
                });
                ui.add_space(4.0);
            }

            // results
            egui::Frame::none()
                .fill(INPUT_BG)
                .rounding(10.0)
                .inner_margin(Margin::same(6.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_height(list_h);
                    let hits: &[Hit] = fd.search.as_ref().map_or(&[], |s| s.hits.as_slice());
                    if hits.is_empty() {
                        let msg = match &fd.search {
                            None => "Задайте условия и нажмите «Найти» или Enter",
                            Some(s) if s.running => "Ищем…",
                            Some(_) => "Ничего не найдено",
                        };
                        let r = ui.max_rect();
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, msg, regular(14.0), MUTED);
                        return;
                    }
                    let mut sa = egui::ScrollArea::vertical().id_salt("find-results").auto_shrink([false, false]);
                    if fd.scroll_to_sel {
                        let y = fd.sel as f32 * ROW;
                        let mut off = fd.list_off;
                        if y < off {
                            off = y;
                        } else if y + ROW > off + fd.list_h {
                            off = y + ROW - fd.list_h;
                        }
                        sa = sa.vertical_scroll_offset(off.max(0.0));
                        fd.scroll_to_sel = false;
                    }
                    let mut clicked = None;
                    let mut row_act = None;
                    let out = sa.show_rows(ui, ROW, hits.len(), |ui, range| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for i in range {
                            let h = &hits[i];
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                            let p = ui.painter();
                            let bg = rect.shrink2(vec2(0.0, 1.0));
                            if i == fd.sel {
                                p.rect_filled(bg, 8.0, SEL_ACTIVE);
                            } else if resp.hovered() {
                                p.rect_filled(bg, 8.0, CARD);
                            }
                            let name = h.name();
                            let ir = Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(24.0, 24.0));
                            if h.is_dir {
                                icons::folder(p, ir);
                            } else {
                                let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
                                crate::file_icons::paint(
                                    (self.settings.associated_icons && h.inner.is_none()).then_some(&mut self.file_icons),
                                    p, ir, &h.path, FileKind::from_name(&name), &ext,
                                );
                            }
                            let right_w = 150.0;
                            let clip = Rect::from_min_max(pos2(rect.left() + 42.0, rect.top()), pos2(rect.right() - right_w - 8.0, rect.bottom()));
                            let pc = p.with_clip_rect(clip);
                            let title_end = pc
                                .text(pos2(clip.left(), rect.center().y - 9.0), Align2::LEFT_CENTER, &name, semibold(14.0), TEXT)
                                .right();
                            if h.inner.is_some() {
                                pc.text(
                                    pos2(title_end + 8.0, rect.center().y - 9.0),
                                    Align2::LEFT_CENTER,
                                    "в архиве",
                                    regular(12.0),
                                    ORANGE,
                                );
                            }
                            let (sub, sub_col) = match &h.snippet {
                                Some(s) => (s.clone(), TEXT_SOFT),
                                None => (h.location(), MUTED),
                            };
                            pc.text(
                                pos2(clip.left(), rect.center().y + 10.0),
                                Align2::LEFT_CENTER,
                                sub,
                                if h.snippet.is_some() { mono(12.0) } else { regular(12.5) },
                                sub_col,
                            );
                            let rx = rect.right() - 12.0;
                            let size = if h.is_dir { "Папка".to_string() } else { format_size(h.size) };
                            p.text(pos2(rx, rect.center().y - 9.0), Align2::RIGHT_CENTER, size, regular(12.5), TEXT_SOFT);
                            p.text(
                                pos2(rx, rect.center().y + 10.0),
                                Align2::RIGHT_CENTER,
                                format_time(h.modified),
                                regular(12.0),
                                MUTED,
                            );

                            let mut tip = h.path.display().to_string();
                            if let Some(inner) = &h.inner {
                                tip.push_str(&format!(" › {}", inner.replace('/', "\\")));
                            }
                            if let Some(s) = &h.snippet {
                                tip.push_str(&format!("\n\n{s}"));
                            }
                            let resp = resp.on_hover_text(tip);
                            if resp.clicked() || resp.secondary_clicked() {
                                clicked = Some(i);
                            }
                            if row_double_clicked(&resp, Id::new(("find-row", &h.path, &h.inner))) {
                                clicked = Some(i);
                                row_act = Some(Act::GoTo);
                            }
                            let in_zip = h.inner.is_some();
                            let is_dir = h.is_dir;
                            resp.context_menu(|ui| {
                                menu_style(ui, 260.0);
                                let items = [
                                    ("Перейти к файлу", "Enter", true, Act::GoTo),
                                    ("Просмотр", "F3", !is_dir && !in_zip, Act::View),
                                    ("Редактировать", "F4", !is_dir && !in_zip, Act::Edit),
                                    ("Открыть", "", true, Act::Open),
                                    ("Копировать путь", "", true, Act::CopyPath),
                                ];
                                for (label, key, en, a) in items {
                                    if menu_item(ui, label, key, en, false).clicked() {
                                        clicked = Some(i);
                                        row_act = Some(a);
                                        ui.close_menu();
                                    }
                                }
                            });
                        }
                    });
                    fd.list_off = out.state.offset.y;
                    fd.list_h = out.inner_rect.height();
                    if let Some(i) = clicked {
                        fd.sel = i;
                    }
                    if row_act.is_some() {
                        act = row_act;
                    }
                });
            ui.add_space(2.0);

            ui.horizontal_wrapped(|ui| {
                let has = n > 0;
                let small = |ui: &mut Ui, text: &str, enabled: bool| -> bool { ui.add_enabled_ui(enabled, |ui| btn(ui, text, INPUT_BG, TEXT)).inner.clicked() };
                if small(ui, "Перейти к файлу", has) {
                    act = Some(Act::GoTo);
                }
                if small(ui, "Просмотр  F3", has) {
                    act = Some(Act::View);
                }
                if small(ui, "Файлы в панель", has) {
                    act = Some(Act::Feed);
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Enter — найти · Esc — закрыть").size(12.0).color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if running {
                        if btn(ui, "Остановить", RED, Color32::WHITE).clicked() {
                            act = Some(Act::Stop);
                        }
                    } else if btn(ui, "Найти", BLUE, Color32::WHITE).clicked() {
                        act = Some(Act::Start);
                    }
                    if btn(ui, "Закрыть", INPUT_BG, TEXT).clicked() {
                        act = Some(Act::Close);
                    }
                    if fd.search.is_some() && !running && btn(ui, "Новый поиск", INPUT_BG, TEXT).clicked() {
                        act = Some(Act::Clear);
                    }
                });
            });
        });

        let Some(act) = act else { return true };
        match act {
            Act::Start => {
                let base = self.ap_ref().path.clone();
                let q = match fd.query(&base) {
                    Ok(q) => q,
                    Err(e) => {
                        fd.error = Some(e);
                        return true;
                    }
                };
                let root = q.roots.first().cloned().unwrap_or_default();
                let label = if q.mask.trim().is_empty() {
                    "Поиск: *".to_string()
                } else {
                    format!("Поиск: {}", q.mask.trim())
                };
                let (mask, text) = (q.mask.clone(), q.text.as_ref().map(|t| t.text.clone()));
                match Search::start(q, ctx.clone()) {
                    Ok(s) => {
                        fd.search = Some(s);
                        fd.search_root = root;
                        fd.search_label = label;
                        fd.error = None;
                        fd.sel = 0;
                        fd.list_off = 0.0;
                        fd.scroll_to_sel = true;
                        ctx.memory_mut(|m| {
                            if let Some(f) = m.focused() {
                                m.surrender_focus(f);
                            }
                        });
                        config::remember(&mut self.settings.find_masks, &mask);
                        if let Some(t) = text {
                            config::remember(&mut self.settings.find_texts, &t);
                        }
                        self.save_settings();
                    }
                    Err(e) => fd.error = Some(e),
                }
                true
            }
            Act::Stop => {
                fd.stop();
                true
            }
            Act::Close => {
                fd.stop();
                false
            }
            Act::Clear => {
                fd.search = None;
                fd.error = None;
                fd.sel = 0;
                fd.focus_mask = true;
                true
            }
            Act::GoTo => {
                let Some(h) = fd.hit() else { return true };
                fd.stop();
                let name = h.path.file_name().map(|n| n.to_string_lossy().into_owned());
                let (dir, select) = match (h.path.parent(), name) {
                    (Some(d), Some(n)) => (d.to_path_buf(), Some(n)),
                    _ => (h.path.clone(), None),
                };
                if let Err(e) = self.ap().navigate(dir) {
                    self.toast(e, true);
                    return true;
                }
                if let Some(n) = select {
                    self.ap().select_name(&n);
                    let found = self.ap_ref().current().is_some_and(|e| e.name.eq_ignore_ascii_case(&n));
                    if !found {
                        self.toast(format!("«{n}» скрыт — включите показ скрытых файлов (F11)"), false);
                    }
                }
                false
            }
            Act::View => {
                let Some(h) = fd.hit() else { return true };
                if h.is_dir || h.inner.is_some() {
                    self.toast("Просмотр доступен только для обычных файлов", true);
                    return true;
                }
                self.modal = load_viewer(h.path);
                self.resume_find = true;
                true
            }
            Act::Edit => {
                let Some(h) = fd.hit() else { return true };
                if h.is_dir || h.inner.is_some() {
                    self.toast("Редактировать можно только обычный файл", true);
                } else if let Err(e) = fsops::open_in_editor(&self.settings.editor, &h.path) {
                    self.toast(format!("Не удалось открыть редактор: {e}"), true);
                }
                true
            }
            Act::Open => {
                let Some(h) = fd.hit() else { return true };
                if let Err(e) = fsops::open_external(&h.path) {
                    self.toast(format!("Не удалось открыть: {e}"), true);
                }
                true
            }
            Act::CopyPath => {
                let Some(h) = fd.hit() else { return true };
                let mut s = h.path.display().to_string();
                if let Some(inner) = &h.inner {
                    s.push_str(&format!(" › {}", inner.replace('/', "\\")));
                }
                ctx.copy_text(s);
                self.toast("Путь скопирован", false);
                true
            }
            Act::Feed => {
                let Some(s) = &fd.search else { return true };
                let mut seen = std::collections::HashSet::new();
                let paths: Vec<PathBuf> = s.hits.iter().map(|h| h.path.clone()).filter(|p| seen.insert(p.clone())).collect();
                let count = paths.len();
                fd.stop();
                let (root, label) = (fd.search_root.clone(), fd.search_label.clone());
                self.ap().show_results(root, paths, label);
                self.toast(format!("В панели: {}. «..» — вернуться в папку", items_text(count as u64)), false);
                false
            }
        }
    }
}
