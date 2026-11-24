//! «Поиск файлов» (Alt+F7) — a popup laid out like JetBrains «Find in Files»: query fields with
//! inline toggles, a dense result list and a preview of the selected file.
//! The search itself runs in `crate::search`; this module holds the form, the live
//! result list and what can be done with a result (go to it, view, edit, feed to panel).

use quokka_domain::{tr, trf};
use super::*;
use crate::config;
use crate::search::{Hit, Kinds, Query, Search, SizeCmp, TextQuery};
use egui::text::{LayoutJob, TextFormat};
use std::path::Path;
use std::time::{Duration, SystemTime};

const ROW: f32 = 32.0;
const PREVIEW_LIMIT: u64 = 512 * 1024;
const PREVIEW_LINES: usize = 5000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DateMode {
    Any,
    Within,
    Between,
}

fn depths() -> [(Option<usize>, &'static str); 7] {
    [
    (None, tr!("Во всех вложенных папках", "In all subfolders")),
    (Some(0), tr!("Только в этой папке", "Only in this folder")),
    (Some(1), tr!("На 1 уровень вглубь", "1 level deep")),
    (Some(2), tr!("На 2 уровня вглубь", "2 levels deep")),
    (Some(3), tr!("На 3 уровня вглубь", "3 levels deep")),
    (Some(5), tr!("На 5 уровней вглубь", "5 levels deep")),
    (Some(10), tr!("На 10 уровней вглубь", "10 levels deep")),
]
}
fn size_cmps() -> [(SizeCmp, &'static str); 3] {
    [(SizeCmp::Less, tr!("меньше", "less than")), (SizeCmp::Equal, tr!("равен", "equal to")), (SizeCmp::Greater, tr!("больше", "greater than"))]
}
fn size_units() -> [(u64, &'static str); 4] {
    [(1, tr!("Б", "B")), (1024, tr!("КБ", "KB")), (1 << 20, tr!("МБ", "MB")), (1 << 30, tr!("ГБ", "GB"))]
}
fn age_units() -> [(u64, &'static str); 6] {
    [
    (60, tr!("минут", "minutes")),
    (3600, tr!("часов", "hours")),
    (86_400, tr!("дней", "days")),
    (604_800, tr!("недель", "weeks")),
    (2_592_000, tr!("месяцев", "months")),
    (31_536_000, tr!("лет", "years")),
]
}

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
    filters_open: bool,
    preview_key: Option<(PathBuf, Option<String>)>,
    preview: Preview,
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
    let d = chrono::NaiveDate::parse_from_str(s, "%d.%m.%Y").map_err(|_| trf!("Дата «{s}» должна быть в виде ДД.ММ.ГГГГ", "Date “{s}” must be in DD.MM.YYYY format"))?;
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
            filters_open: false,
            preview_key: None,
            preview: Preview::default(),
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
            let v: u64 = self.size_val.trim().parse().map_err(|_| tr!("Размер — целое неотрицательное число", "Size must be a non-negative integer").to_string())?;
            Some((self.size_cmp, v, size_units()[self.size_unit].0))
        } else {
            None
        };
        let (mut newer_than, mut older_than) = (None, None);
        match self.date_mode {
            DateMode::Any => {}
            DateMode::Within => {
                let n: u64 = self.age_val.trim().parse().map_err(|_| tr!("Возраст файла — целое число", "File age must be an integer").to_string())?;
                newer_than = SystemTime::now().checked_sub(Duration::from_secs(n.saturating_mul(age_units()[self.age_unit].0)));
            }
            DateMode::Between => {
                newer_than = parse_date(&self.date_from, false)?;
                older_than = parse_date(&self.date_to, true)?;
                if newer_than.is_none() && older_than.is_none() {
                    return Err(tr!("Укажите хотя бы одну дату", "Specify at least one date").into());
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

/// Text of the selected result for the preview pane.
#[derive(Default)]
struct Preview {
    lines: Vec<String>,
    /// Line with the match, scrolled into view.
    focus: Option<usize>,
    /// Shown instead of the text (folders, archives, binary files).
    note: Option<&'static str>,
    scroll: bool,
}

fn load_preview(h: &Hit, needle: &str, case: bool) -> Preview {
    let mut pv = Preview { scroll: true, ..Default::default() };
    if h.is_dir {
        pv.note = Some(tr!("Папка", "Folder"));
        return pv;
    }
    if h.inner.is_some() {
        pv.note = Some(tr!("Файл внутри ZIP-архива — предпросмотр недоступен", "File inside a ZIP archive — preview unavailable"));
        return pv;
    }
    let mut buf = Vec::new();
    match std::fs::File::open(&h.path) {
        Ok(f) => {
            let _ = f.take(PREVIEW_LIMIT).read_to_end(&mut buf);
        }
        Err(_) => {
            pv.note = Some(tr!("Не удалось открыть файл", "Failed to open the file"));
            return pv;
        }
    }
    if buf.iter().take(8192).any(|&b| b == 0) {
        // binary: the preview simply stays empty
        return pv;
    }
    pv.lines = String::from_utf8_lossy(&buf)
        .lines()
        .take(PREVIEW_LINES)
        .map(|l| l.chars().take(600).collect::<String>().replace('\t', "    "))
        .collect();
    if pv.lines.is_empty() {
        pv.note = Some(tr!("Пустой файл", "Empty file"));
    }
    let fold = |s: &str| if case { s.to_string() } else { s.to_lowercase() };
    if !needle.is_empty() {
        let n = fold(needle);
        pv.focus = pv.lines.iter().position(|l| fold(l).contains(&n));
    }
    if pv.focus.is_none() {
        // regex / "not containing": fall back to the line the search reported
        if let Some(sn) = &h.snippet {
            let core: String = sn.trim_start_matches('…').chars().take(40).collect();
            let core = core.trim();
            if !core.is_empty() {
                pv.focus = pv.lines.iter().position(|l| l.split_whitespace().collect::<Vec<_>>().join(" ").contains(core));
            }
        }
    }
    pv
}

/// A preview line with every occurrence of `needle` marked like an IDE search hit.
fn highlight_line(line: &str, needle: &str, case: bool, font: egui::FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let normal = TextFormat { font_id: font.clone(), color: TEXT(), ..Default::default() };
    let hit = TextFormat { font_id: font, color: TEXT(), background: SEARCH_HIT(), ..Default::default() };
    let (hay, nd) = if case { (line.to_string(), needle.to_string()) } else { (line.to_lowercase(), needle.to_lowercase()) };
    let mut i = 0;
    if !nd.is_empty() && hay.len() == line.len() {
        while let Some(off) = hay[i..].find(&nd) {
            let (st, en) = (i + off, i + off + nd.len());
            if !line.is_char_boundary(st) || !line.is_char_boundary(en) {
                break;
            }
            job.append(&line[i..st], 0.0, normal.clone());
            job.append(&line[st..en], 0.0, hit.clone());
            i = en;
        }
    }
    job.append(&line[i..], 0.0, normal);
    job
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
    pub(super) fn find_ui(&mut self, ctx: &egui::Context, fd: &mut FindDialog, esc: bool, x_close: bool, enter: bool) -> bool {
        if let Some(s) = &mut fd.search {
            s.poll();
            if s.running {
                ctx.request_repaint_after(Duration::from_millis(120));
            }
        }
        let running = fd.search.as_ref().is_some_and(|s| s.running);
        let n = fd.search.as_ref().map_or(0, |s| s.hits.len());
        fd.sel = fd.sel.min(n.saturating_sub(1));
        let typing = ctx.egui_wants_keyboard_input();
        let mut act = None;

        if x_close {
            // the corner cross always closes; Esc first stops a running search
            act = Some(Act::Close);
        } else if esc {
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

        let screen = ctx.content_rect();
        let width = (screen.width() - 80.0).min(1000.0);
        let total_h = (screen.height() - 60.0).min(880.0);
        let masks = self.settings.find_masks.clone();
        let texts = self.settings.find_texts.clone();
        let needle = (fd.text_on && !fd.text_regex && !fd.text_not).then(|| fd.text.clone()).unwrap_or_default();

        // the preview follows the selection
        let key = fd.hit().map(|h| (h.path.clone(), h.inner.clone()));
        if key != fd.preview_key {
            fd.preview_key = key;
            fd.preview = fd.hit().map_or_else(Preview::default, |h| load_preview(&h, &needle, fd.text_case));
        }

        flat_dialog_own_close(ctx, "find", width, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            const PAD: f32 = 24.0;

            // ── title bar: name, live status, filters and close
            egui::Frame::NONE.inner_margin(Margin { left: PAD as i8, right: 16, top: 16, bottom: 14 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.label(RichText::new(tr!("Поиск файлов", "Find files")).font(semibold(14.5)).color(TEXT()));
                    match &fd.search {
                        Some(s) => {
                            if s.running {
                                ui.add(egui::Spinner::new().size(13.0).color(BLUE));
                            }
                            let mut status = if s.running {
                                trf!("найдено {n} · просмотрено файлов: {}, папок: {}", "found {n} · files scanned: {}, folders: {}", s.files, s.dirs)
                            } else {
                                trf!(
                                    "{} {n} за {:.1} с · просмотрено файлов: {}, папок: {}", "{} {n} in {:.1} s · files scanned: {}, folders: {}",
                                    if s.cancelled { tr!("остановлено, найдено", "stopped, found") } else { tr!("найдено", "found") },
                                    s.elapsed.as_secs_f32(),
                                    s.files,
                                    s.dirs
                                )
                            };
                            if s.errors > 0 {
                                status.push_str(&trf!(" · недоступно: {}", " · inaccessible: {}", s.errors));
                            }
                            ui.label(RichText::new(status).size(12.5).color(MUTED()));
                        }
                        None => {
                            ui.label(RichText::new(tr!("по имени, содержимому и свойствам · Alt+F7", "by name, content and properties · Alt+F7")).size(12.5).color(MUTED()));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if jb_tool(ui, Glyph::Close, true, tr!("Закрыть (Esc)", "Close (Esc)")).clicked() {
                            act = Some(Act::Close);
                        }
                        let k = fd.advanced_count();
                        let label = if k > 0 { trf!("Фильтры · {k}", "Filters · {k}") } else { tr!("Фильтры", "Filters").to_string() };
                        if field_toggle(ui, &label, fd.filters_open, tr!("Размер и дата изменения", "Size and date modified")).clicked() {
                            fd.filters_open = !fd.filters_open;
                        }
                    });
                });
            });

            // ── query: name, text, scope, filters
            egui::Frame::NONE.inner_margin(Margin { left: PAD as i8, right: PAD as i8, top: 0, bottom: 18 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                ui.add_enabled_ui(!running, |ui| {
                    let hint = if fd.mask_regex { tr!(r"Имя (regex): ^IMG_\d{4}\.jpe?g$", r"Name (regex): ^IMG_\d{4}\.jpe?g$") } else { tr!("Имя: *.txt;*.doc | temp*   — пусто: все файлы", "Name: *.txt;*.doc | temp*   — empty: all files") };
                    let font = if fd.mask_regex { mono(14.0) } else { regular(14.0) };
                    let (mut out, _) = jb_field(ui, Id::unique("find-mask"), &mut fd.mask, hint, Glyph::Search, font, &masks, |ui| {
                        if field_toggle(ui, ".*", fd.mask_regex, tr!("Регулярное выражение вместо масок", "Regular expression instead of masks")).clicked() {
                            fd.mask_regex = !fd.mask_regex;
                        }
                        if field_toggle(ui, "Aa", fd.mask_case, tr!("Учитывать регистр в имени", "Match case in name")).clicked() {
                            fd.mask_case = !fd.mask_case;
                        }
                    });
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
                    let tip = if fd.mask_regex {
                        tr!("Регулярное выражение ищется в имени файла (без пути).", "The regular expression is matched against the file name (without path).")
                    } else {
                        tr!("Маски через «;» или пробел; без * и ? ищется часть имени. После «|» — исключения: файлы (*.bak) и папки (node_modules\\).", "Masks separated by “;” or space; without * and ? part of the name is searched. After “|” — exclusions: files (*.bak) and folders (node_modules\\).")
                    };
                    out.response.response.clone().on_hover_text(tip);

                    let font = if fd.text_regex { mono(14.0) } else { regular(14.0) };
                    let (out, _) = jb_field(ui, Id::unique("find-text"), &mut fd.text, tr!("Текст внутри файла — необязательно", "Text inside the file — optional"), Glyph::Document, font, &texts, |ui| {
                        for (on, label, tip) in [
                            (&mut fd.text_enc, "1251", tr!("Искать ещё и в кодировках UTF-16 LE и Windows-1251 (не для регулярных выражений)", "Also search in UTF-16 LE and Windows-1251 encodings (not for regular expressions)")),
                            (&mut fd.text_not, "≠", tr!("Найти файлы, в которых этого текста НЕТ", "Find files that do NOT contain this text")),
                            (&mut fd.text_regex, ".*", tr!("Регулярное выражение; ^ и $ — начало и конец строки", "Regular expression; ^ and $ — start and end of line")),
                            (&mut fd.text_words, "W", tr!("Слова целиком: «кот» не найдётся в «который»", "Whole words: “cat” won’t match “category”")),
                            (&mut fd.text_case, "Aa", tr!("Учитывать регистр", "Match case")),
                        ] {
                            if field_toggle(ui, label, *on, tip).clicked() {
                                *on = !*on;
                            }
                        }
                    });
                    fd.text_on = !fd.text.is_empty();
                    if out.response.lost_focus() && enter {
                        act = Some(Act::Start);
                    }

                    // scope
                    ui.horizontal_wrapped(|ui| {
                        let w = (ui.available_width() * 0.4).max(220.0);
                        ui.allocate_ui(vec2(w, 32.0), |ui| {
                            let (o, _) = jb_field(ui, Id::unique("find-roots"), &mut fd.roots, tr!("Папка; несколько — через «;»", "Folder; several — separated by “;”"), Glyph::Folder, regular(13.5), &[], |_| {});
                            if o.response.lost_focus() && enter {
                                act = Some(Act::Start);
                            }
                        });
                        let cur = depths().iter().find(|(d, _)| *d == fd.depth).map_or("", |(_, l)| l);
                        egui::ComboBox::from_id_salt("find-depth").selected_text(cur).width(200.0).show_ui(ui, |ui| {
                            for (d, l) in depths() {
                                ui.selectable_value(&mut fd.depth, d, l);
                            }
                        });
                        const KINDS: [Kinds; 3] = [Kinds::All, Kinds::Files, Kinds::Dirs];
                        let k = KINDS.iter().position(|k| *k == fd.kinds).unwrap_or(0);
                        if let Some(i) = segmented(ui, &[tr!("Всё", "All"), tr!("Файлы", "Files"), tr!("Папки", "Folders")], k) {
                            fd.kinds = KINDS[i];
                        }
                        ui.checkbox(&mut fd.hidden, RichText::new(tr!("Скрытые", "Hidden")).color(TEXT())).on_hover_text(tr!("Искать среди скрытых и системных файлов", "Search among hidden and system files"));
                        ui.checkbox(&mut fd.archives, RichText::new(tr!("В ZIP", "In ZIP")).color(TEXT()))
                            .on_hover_text(tr!("Искать среди имён файлов в ZIP-архивах (текст внутри архивов не ищется)", "Search among file names in ZIP archives (text inside archives is not searched)"));
                    });

                    if fd.filters_open {
                        egui::Frame::NONE.fill(NAV_BG()).stroke(Stroke::new(1.0, BORDER())).corner_radius(rad(6.0)).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                            ui.horizontal(|ui| {
                                ui.add_sized(vec2(120.0, 26.0), egui::Checkbox::new(&mut fd.size_on, RichText::new(tr!("Размер", "Size")).color(TEXT())));
                                ui.add_enabled_ui(fd.size_on, |ui| {
                                    let cur = size_cmps().iter().find(|(c, _)| *c == fd.size_cmp).map_or("", |(_, l)| l);
                                    egui::ComboBox::from_id_salt("find-size-cmp").selected_text(cur).width(96.0).show_ui(ui, |ui| {
                                        for (c, l) in size_cmps() {
                                            ui.selectable_value(&mut fd.size_cmp, c, l);
                                        }
                                    });
                                    ui.add(egui::TextEdit::singleline(&mut fd.size_val).desired_width(70.0).margin(vec2(6.0, 4.0)).hint_text("100"));
                                    egui::ComboBox::from_id_salt("find-size-unit").selected_text(size_units()[fd.size_unit].1).width(60.0).show_ui(ui, |ui| {
                                        for (i, (_, l)) in size_units().iter().enumerate() {
                                            ui.selectable_value(&mut fd.size_unit, i, *l);
                                        }
                                    });
                                });
                                if fd.size_on && fd.size_val.trim().is_empty() {
                                    fd.size_val = "100".into();
                                }
                                if fd.size_on {
                                    ui.label(RichText::new(tr!("папки не показываются", "folders are not shown")).size(12.0).color(MUTED()));
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.add_sized(vec2(120.0, 26.0), egui::Label::new(RichText::new(tr!("Дата изменения", "Date modified")).color(TEXT())));
                                const MODES: [DateMode; 3] = [DateMode::Any, DateMode::Within, DateMode::Between];
                                let k = MODES.iter().position(|m| *m == fd.date_mode).unwrap_or(0);
                                if let Some(i) = segmented(ui, &[tr!("Любая", "Any"), tr!("Не старше", "Not older than"), tr!("Промежуток", "Range")], k) {
                                    fd.date_mode = MODES[i];
                                }
                                ui.add_space(6.0);
                                match fd.date_mode {
                                    DateMode::Any => {}
                                    DateMode::Within => {
                                        ui.add(egui::TextEdit::singleline(&mut fd.age_val).desired_width(50.0).margin(vec2(6.0, 4.0)));
                                        egui::ComboBox::from_id_salt("find-age-unit").selected_text(age_units()[fd.age_unit].1).width(90.0).show_ui(ui, |ui| {
                                            for (i, (_, l)) in age_units().iter().enumerate() {
                                                ui.selectable_value(&mut fd.age_unit, i, *l);
                                            }
                                        });
                                    }
                                    DateMode::Between => {
                                        ui.label(RichText::new(tr!("с", "from")).color(TEXT()));
                                        ui.add(egui::TextEdit::singleline(&mut fd.date_from).desired_width(100.0).margin(vec2(6.0, 4.0)).hint_text(tr!("ДД.ММ.ГГГГ", "DD.MM.YYYY")));
                                        ui.label(RichText::new(tr!("по", "to")).color(TEXT()));
                                        ui.add(egui::TextEdit::singleline(&mut fd.date_to).desired_width(100.0).margin(vec2(6.0, 4.0)).hint_text(tr!("ДД.ММ.ГГГГ", "DD.MM.YYYY")));
                                        ui.label(RichText::new(tr!("любую дату можно опустить", "either date can be omitted")).size(12.0).color(MUTED()));
                                    }
                                }
                            });
                        });
                    }
                });
                if let Some(e) = &fd.error {
                    ui.label(RichText::new(e.as_str()).color(RED).size(13.0));
                }
            });

            let rest = (total_h - ui.min_rect().height() - 60.0).max(300.0);
            let list_h = (rest * 0.68).floor();
            let prev_h = rest - list_h;

            // ── results: one dense line per hit, location on the right
            let (lr, _) = ui.allocate_exact_size(vec2(width, list_h), Sense::hover());
            ui.painter().hline(lr.x_range(), lr.top(), Stroke::new(1.0, BORDER()));
            place(ui, egui::UiBuilder::new().max_rect(lr.shrink2(vec2(0.0, 1.0))), |ui| {
                let hits: &[Hit] = fd.search.as_ref().map_or(&[], |s| s.hits.as_slice());
                if hits.is_empty() {
                    let msg = match &fd.search {
                        None => tr!("Задайте условия и нажмите Enter", "Set the conditions and press Enter"),
                        Some(s) if s.running => tr!("Ищем…", "Searching…"),
                        Some(_) => tr!("Ничего не найдено", "Nothing found"),
                    };
                    ui.painter().text(lr.center(), Align2::CENTER_CENTER, msg, regular(13.5), MUTED());
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
                        if i == fd.sel {
                            p.rect_filled(rect, 0.0, NAV_SEL());
                        } else if resp.hovered() {
                            p.rect_filled(rect, 0.0, HOVER());
                        }
                        let cy = rect.center().y;
                        let name = h.name();
                        let ir = Rect::from_center_size(pos2(rect.left() + PAD + 8.0, cy), vec2(16.0, 16.0));
                        if h.is_dir {
                            icons::folder(p, ir);
                        } else {
                            let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
                            crate::file_icons::paint(
                                (self.settings.associated_icons && h.inner.is_none()).then_some(&mut self.file_icons),
                                p, ir, &h.path, FileKind::from_name(&name), &ext,
                            );
                        }
                        let small = regular(12.5);
                        let size_x = rect.right() - PAD;
                        if !h.is_dir {
                            p.text(pos2(size_x, cy), Align2::RIGHT_CENTER, format_size(h.size), small.clone(), MUTED());
                        }
                        let loc = elide_left(p, &h.location(), &small, rect.width() * 0.38);
                        let loc_r = p.text(pos2(size_x - 78.0, cy), Align2::RIGHT_CENTER, loc, small, MUTED());
                        let clip = Rect::from_min_max(pos2(ir.right() + 8.0, rect.top()), pos2(loc_r.left() - 14.0, rect.bottom()));
                        let pc = p.with_clip_rect(clip);
                        let mut end = pc.text(pos2(clip.left(), cy), Align2::LEFT_CENTER, &name, regular(13.5), TEXT()).right();
                        if h.inner.is_some() {
                            end = pc.text(pos2(end + 8.0, cy), Align2::LEFT_CENTER, tr!("в архиве", "in archive"), regular(12.0), ORANGE).right();
                        }
                        if let Some(s) = &h.snippet {
                            pc.text(pos2(end + 14.0, cy), Align2::LEFT_CENTER, s, mono(12.5), TEXT_SOFT());
                        }

                        let mut tip = h.path.display().to_string();
                        if let Some(inner) = &h.inner {
                            tip.push_str(&format!(" › {}", inner.replace('/', "\\")));
                        }
                        let resp = resp.on_hover_text(tip);
                        if resp.clicked() || resp.secondary_clicked() {
                            clicked = Some(i);
                        }
                        if row_double_clicked(&resp, Id::unique(("find-row", &h.path, &h.inner))) {
                            clicked = Some(i);
                            row_act = Some(Act::GoTo);
                        }
                        let in_zip = h.inner.is_some();
                        let is_dir = h.is_dir;
                        resp.context_menu(|ui| {
                            menu_style(ui, 260.0);
                            let items = [
                                (Glyph::ArrowRight, tr!("Перейти к файлу", "Go to file"), "Enter", true, Act::GoTo),
                                (Glyph::Eye, tr!("Просмотр", "View"), "F3", !is_dir && !in_zip, Act::View),
                                (Glyph::Pencil, tr!("Редактировать", "Edit"), "F4", !is_dir && !in_zip, Act::Edit),
                                (Glyph::Document, tr!("Открыть", "Open"), "", true, Act::Open),
                                (Glyph::Copy, tr!("Копировать путь", "Copy path"), "", true, Act::CopyPath),
                            ];
                            for (g, label, key, en, a) in items {
                                if menu_item_icon(ui, Some(g), label, key, en, false).clicked() {
                                    clicked = Some(i);
                                    row_act = Some(a);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
                fd.list_off = out.state.clamped_offset().y;
                fd.list_h = out.inner_rect.height();
                if let Some(i) = clicked {
                    if i != fd.sel {
                        fd.sel = i;
                    }
                }
                if row_act.is_some() {
                    act = row_act;
                }
            });

            // ── preview of the selected file, scrolled to the match
            let (pr, _) = ui.allocate_exact_size(vec2(width, prev_h), Sense::hover());
            let head = Rect::from_min_size(pr.min, vec2(width, 32.0));
            ui.painter().rect_filled(head, 0.0, NAV_BG());
            ui.painter().hline(pr.x_range(), pr.top(), Stroke::new(1.0, BORDER()));
            ui.painter().hline(pr.x_range(), head.bottom(), Stroke::new(1.0, BORDER()));
            if let Some(h) = fd.hit() {
                let p = ui.painter();
                let small = regular(12.5);
                let info = if h.is_dir { format_time(h.modified) } else { format!("{} · {}", format_size(h.size), format_time(h.modified)) };
                let ir = p.text(pos2(head.right() - PAD, head.center().y), Align2::RIGHT_CENTER, info, small.clone(), MUTED());
                let mut path = h.path.display().to_string();
                if let Some(inner) = &h.inner {
                    path.push_str(&format!(" › {}", inner.replace('/', "\\")));
                }
                let path = elide_left(p, &path, &small, ir.left() - head.left() - PAD - 40.0);
                p.text(pos2(head.left() + PAD, head.center().y), Align2::LEFT_CENTER, path, small, TEXT_SOFT());
            }
            let body = Rect::from_min_max(pos2(pr.left(), head.bottom() + 1.0), pr.max);
            place(ui, egui::UiBuilder::new().max_rect(body), |ui| {
                let pv = &mut fd.preview;
                if pv.lines.is_empty() {
                    let msg = pv.note.unwrap_or(if fd.preview_key.is_some() { "" } else { tr!("Выберите результат, чтобы увидеть содержимое", "Select a result to see its contents") });
                    ui.painter().text(body.center(), Align2::CENTER_CENTER, msg, regular(13.0), MUTED());
                    return;
                }
                const LINE: f32 = 20.0;
                const GUTTER: f32 = 56.0;
                let mut sa = egui::ScrollArea::vertical().id_salt(("find-preview", fd.preview_key.clone())).auto_shrink([false, false]);
                if pv.scroll {
                    pv.scroll = false;
                    if let Some(f) = pv.focus {
                        sa = sa.vertical_scroll_offset((f as f32 * LINE - body.height() / 3.0).max(0.0));
                    }
                }
                ui.painter().rect_filled(Rect::from_min_size(body.min, vec2(GUTTER, body.height())), CornerRadius::ZERO, NAV_BG());
                sa.show_rows(ui, LINE, pv.lines.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for i in range {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), LINE), Sense::hover());
                        let p = ui.painter();
                        if pv.focus == Some(i) {
                            p.rect_filled(Rect::from_min_max(pos2(r.left() + GUTTER, r.top()), r.max), 0.0, BLUE_SOFT());
                        }
                        p.text(pos2(r.left() + GUTTER - 10.0, r.center().y), Align2::RIGHT_CENTER, (i + 1).to_string(), mono(12.0), MUTED());
                        let job = highlight_line(&pv.lines[i], &needle, fd.text_case, mono(13.0));
                        let g = ui.fonts_mut(|f| f.layout_job(job));
                        let pc = p.with_clip_rect(Rect::from_min_max(pos2(r.left() + GUTTER + 4.0, r.top()), r.max));
                        pc.galley(pos2(r.left() + GUTTER + 10.0, r.center().y - g.size().y / 2.0), g, TEXT());
                    }
                });
            });

            // ── footer
            let (bar, _) = ui.allocate_exact_size(vec2(width, 60.0), Sense::hover());
            ui.painter().rect_filled(bar, CornerRadius { sw: rad_u8(FLAT_R as u8), se: rad_u8(FLAT_R as u8), ..Default::default() }, NAV_BG());
            ui.painter().hline(bar.x_range(), bar.top(), Stroke::new(1.0, BORDER()));
            place(ui, egui::UiBuilder::new().max_rect(bar.shrink2(vec2(PAD, 0.0))).layout(Layout::left_to_right(Align::Center)), |ui| {
                ui.label(RichText::new(tr!("Enter — перейти · F3 — просмотр · F4 — редактор · Esc — закрыть", "Enter — go to · F3 — view · F4 — editor · Esc — close")).size(12.0).color(MUTED()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if running {
                        if jb_btn(ui, tr!("Остановить", "Stop"), false).clicked() {
                            act = Some(Act::Stop);
                        }
                    } else if jb_btn(ui, tr!("Найти", "Find"), true).clicked() {
                        act = Some(Act::Start);
                    }
                    if ui.add_enabled_ui(n > 0, |ui| jb_btn(ui, tr!("Файлы в панель", "Files to pane"), false)).inner.on_hover_text(tr!("Показать все результаты в текущей панели", "Show all results in the current pane")).clicked() {
                        act = Some(Act::Feed);
                    }
                    if fd.search.is_some() && !running && jb_btn(ui, tr!("Новый поиск", "New search"), false).clicked() {
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
                    tr!("Поиск: *", "Search: *").to_string()
                } else {
                    trf!("Поиск: {}", "Search: {}", q.mask.trim())
                };
                let (mask, text) = (q.mask.clone(), q.text.as_ref().map(|t| t.text.clone()));
                match Search::start(q, super::waker(ctx)) {
                    Ok(s) => {
                        fd.search = Some(s);
                        fd.search_root = root;
                        fd.search_label = label;
                        fd.error = None;
                        fd.sel = 0;
                        fd.preview_key = None;
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
                        self.toast(trf!("«{n}» скрыт — включите показ скрытых файлов (F11)", "“{n}” is hidden — turn on showing hidden files (F11)"), false);
                    }
                }
                false
            }
            Act::View => {
                let Some(h) = fd.hit() else { return true };
                if h.is_dir || h.inner.is_some() {
                    self.toast(tr!("Просмотр доступен только для обычных файлов", "Viewing is available only for regular files"), true);
                    return true;
                }
                self.modal = load_viewer(h.path);
                self.resume_find = true;
                true
            }
            Act::Edit => {
                let Some(h) = fd.hit() else { return true };
                if h.is_dir || h.inner.is_some() {
                    self.toast(tr!("Редактировать можно только обычный файл", "Only a regular file can be edited"), true);
                } else if let Err(e) = fsops::open_in_editor(&self.settings.editor, &h.path) {
                    self.toast(trf!("Не удалось открыть редактор: {e}", "Failed to open the editor: {e}"), true);
                }
                true
            }
            Act::Open => {
                let Some(h) = fd.hit() else { return true };
                if let Err(e) = fsops::open_external(&h.path) {
                    self.toast(trf!("Не удалось открыть: {e}", "Failed to open: {e}"), true);
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
                self.toast(tr!("Путь скопирован", "Path copied"), false);
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
                self.toast(trf!("В панели: {}. «..» — вернуться в папку", "In pane: {}. “..” — back to the folder", items_text(count as u64)), false);
                false
            }
        }
    }
}
