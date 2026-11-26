//! Application menu bar (Файл, Правка, Вид, Переход, Сервис, Справка) that lists every
//! command with its shortcut, and the «О программе» dialog.

use quokka_domain::{tr, trf};
use super::*;

const BAR_H: f32 = 30.0;
const CHANGELOG: &str = include_str!("../../CHANGELOG.md");

/// What the menu bar asked for that is not a plain [`Action`].
#[derive(Default)]
struct BarOut {
    actions: Vec<Action>,
    sort: Option<SortKey>,
    toggle_regex: bool,
    toggle_case: bool,
}

impl BarOut {
    /// A menu row that runs `a` and closes the menu.
    fn item(&mut self, ui: &mut Ui, g: Option<Glyph>, text: &str, key: &str, enabled: bool, a: Action) {
        self.item_ex(ui, g, text, key, enabled, false, a);
    }

    fn item_ex(&mut self, ui: &mut Ui, g: Option<Glyph>, text: &str, key: &str, enabled: bool, danger: bool, a: Action) {
        let key = super::hotkeys::menu_key(&a, key);
        if menu_item_icon(ui, g, text, &key, enabled, danger).clicked() {
            self.actions.push(a);
            ui.close();
        }
    }

    /// A menu row with a check mark in the icon column while `on`.
    fn check(&mut self, ui: &mut Ui, text: &str, key: &str, on: bool, a: Action) {
        self.item(ui, on.then_some(Glyph::Check), text, key, true, a);
    }
}

/// One top-level entry of the bar with its drop-down.
fn top_menu(ui: &mut Ui, title: &str, width: f32, add: impl FnOnce(&mut Ui)) {
    ui.menu_button(RichText::new(title).font(regular(13.5)).color(TEXT()), |ui| {
        menu_style(ui, width);
        menu_scroll(ui, add);
    });
}

impl QuokkaApp {
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let (has_cur, has_targets, marked, is_pc, can_back, can_fwd, can_up, tabs, sort, asc, sizing) = {
            let p = self.ap_ref();
            let cur = p.current().is_some_and(|e| !e.is_parent);
            (
                cur,
                !p.targets().is_empty(),
                !p.marked.is_empty(),
                p.is_pc(),
                !p.back.is_empty(),
                !p.forward.is_empty(),
                parent_of(&p.path).is_some(),
                self.sides[self.active].tabs.len(),
                p.sort,
                p.asc,
                !p.folder_sizes.pending.is_empty(),
            )
        };
        let has_undo = self.undo.is_some();
        let on_server = quokka_domain::remote::is_remote(&self.ap_ref().path) && !quokka_domain::remote::is_device(&self.ap_ref().path);
        let on_sftp = super::remote::is_sftp(&self.ap_ref().path);
        let (dual, hidden, queue_open) = (self.settings.dual, self.settings.show_hidden, self.queue_open);
        let quick_view = self.quick_view.is_some();
        let (regex, case) = (self.settings.regex, self.settings.case_sensitive);
        let mut out = BarOut::default();

        // Whole physical pixels: at 125 % a 30 pt bar ends mid-pixel, and the dark window
        // background shows through the half-covered row as a grey line under the menu.
        let ppp = ctx.pixels_per_point();
        let bar_h = (BAR_H * ppp).round() / ppp;
        egui::Panel::top("menubar")
            .exact_size(bar_h)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(CARD()).inner_margin(Margin::symmetric(10, 0)))
            .show(ui, |ui| {
                // one physical pixel in the usual border colour along the bottom edge
                let r = ui.max_rect().expand2(vec2(10.0, 0.0));
                let px = 1.0 / ppp;
                ui.painter().hline(r.x_range(), r.bottom() - px / 2.0, Stroke::new(px, BORDER()));
                ui.horizontal_centered(|ui| {
                    let s = ui.style_mut();
                    s.spacing.item_spacing.x = 2.0;
                    s.spacing.button_padding = vec2(10.0, 4.0);
                    let w = &mut s.visuals.widgets;
                    for wv in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                        wv.corner_radius = CornerRadius::same(rad_u8(6));
                        wv.bg_stroke = Stroke::NONE;
                        wv.expansion = 0.0;
                    }
                    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
                    w.hovered.weak_bg_fill = HOVER();
                    w.active.weak_bg_fill = BLUE_SOFT();
                    w.open.weak_bg_fill = BLUE_SOFT();

                    // egui switches between open menus on hover only inside a MenuBar;
                    // the empty style modifier keeps the bar look set up above.
                    egui::MenuBar::new().style(egui::style::StyleModifier::default()).ui(ui, |ui| {
                    top_menu(ui, tr!("Файл", "File"), 300.0, |ui| {
                        out.item(ui, Some(Glyph::Folder), tr!("Открыть", "Open"), "Enter", has_cur, Action::Open);
                        out.item(ui, Some(Glyph::Eye), tr!("Просмотр", "View"), "F3", has_cur, Action::View);
                        out.item(ui, Some(Glyph::Pencil), tr!("Правка во внешнем редакторе", "Edit in external editor"), "F4", has_cur, Action::Edit);
                        out.item(ui, None, tr!("Правка во встроенном редакторе", "Edit in built-in editor"), "Shift+F4", has_cur, Action::EditBuiltin);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::FolderPlus), tr!("Новая папка", "New folder"), "F7", !is_pc, Action::MkDir);
                        out.item(ui, Some(Glyph::Document), tr!("Новый файл", "New file"), "F9", !is_pc, Action::NewFile);
                        menu_sep(ui);
                        out.item(ui, None, tr!("Переименовать", "Rename"), "F2", has_cur, Action::Rename);
                        out.item(ui, None, tr!("Групповое переименование…", "Multi-rename…"), "Ctrl+M", has_targets, Action::MultiRename);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Copy), tr!("Копировать в другую панель…", "Copy to other pane…"), "F5", has_targets, Action::Copy);
                        out.item(ui, Some(Glyph::ArrowRight), tr!("Переместить в другую панель…", "Move to other pane…"), "F6", has_targets, Action::Move);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Archive), tr!("Создать ZIP-архив…", "Create ZIP archive…"), "Alt+F5", has_targets, Action::Zip);
                        out.item(ui, Some(Glyph::Image), tr!("Конвертировать изображения…", "Convert images…"), "Ctrl+Shift+E", has_targets, Action::ConvertImages);
                        out.item(ui, Some(Glyph::Unpack), tr!("Распаковать ZIP…", "Extract ZIP…"), "Alt+F6", has_targets, Action::Unzip);
                        menu_sep(ui);
                        out.item_ex(ui, Some(Glyph::Trash), tr!("Удалить в корзину", "Move to Recycle Bin"), "F8 / Del", has_targets, true, Action::Delete);
                        out.item_ex(ui, None, tr!("Удалить навсегда", "Delete permanently"), "Shift+Del", has_targets, true, Action::DeletePermanent);
                        menu_sep(ui);
                        out.item(ui, None, tr!("Открыть в Проводнике", "Open in Explorer"), "", !is_pc, Action::OpenInExplorer);
                        if cfg!(windows) {
                            out.item(ui, Some(Glyph::Info), tr!("Свойства", "Properties"), "Ctrl+Alt+Enter", has_cur || !is_pc, Action::Properties);
                        }
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Close), tr!("Выход", "Exit"), "F10", true, Action::Quit);
                    });

                    top_menu(ui, tr!("Правка", "Edit"), 260.0, |ui| {
                        out.item(ui, None, tr!("Отменить", "Undo"), "Ctrl+Z", has_undo, Action::UndoDelete);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Copy), tr!("Копировать", "Copy"), "Ctrl+C", has_targets, Action::ClipboardCopy);
                        out.item(ui, Some(Glyph::Scissors), tr!("Вырезать", "Cut"), "Ctrl+X", has_targets, Action::ClipboardCut);
                        out.item(ui, Some(Glyph::Clipboard), tr!("Вставить", "Paste"), "Ctrl+V", !is_pc, Action::ClipboardPaste);
                        menu_sep(ui);
                        out.item(ui, None, tr!("Отметить", "Mark"), "Space", has_cur, Action::Mark);
                        out.item(ui, None, tr!("Отметить и вниз", "Mark and move down"), "Insert", has_cur, Action::MarkMove);
                        out.item(ui, Some(Glyph::Check), tr!("Выделить всё", "Select all"), "Ctrl+A", true, Action::MarkAll);
                        out.item(ui, Some(Glyph::Minus), tr!("Снять выделение", "Clear selection"), "Esc", marked, Action::ClearMarks);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Check), tr!("Выделить по маске…", "Select by mask…"), "Ctrl+S", true, Action::SelectMask);
                        out.item(ui, Some(Glyph::Minus), tr!("Снять выделение по маске…", "Deselect by mask…"), "Ctrl+Shift+S", true, Action::UnselectMask);
                        out.item(ui, None, tr!("Инвертировать выделение", "Invert selection"), "Ctrl+Shift+I", true, Action::InvertSelection);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Search), tr!("Поиск по имени…", "Search by name…"), "Ctrl+F", true, Action::FocusSearch);
                        if menu_item_icon(ui, case.then_some(Glyph::Check), tr!("Учитывать регистр", "Match case"), "", true, false).clicked() {
                            out.toggle_case = true;
                            ui.close();
                        }
                        if menu_item_icon(ui, regex.then_some(Glyph::Check), tr!("Регулярные выражения", "Regular expressions"), "Alt+R", true, false).clicked() {
                            out.toggle_regex = true;
                            ui.close();
                        }
                        out.item(ui, Some(Glyph::Search), tr!("Найти файлы…", "Find files…"), "Alt+F7", true, Action::FindFiles);
                    });

                    top_menu(ui, tr!("Вид", "View"), 270.0, |ui| {
                        out.check(ui, tr!("Две панели", "Two panes"), "F12", dual, Action::ToggleDual);
                        out.check(ui, tr!("Быстрый просмотр", "Quick view"), "Ctrl+Q", quick_view, Action::QuickView);
                        out.check(ui, tr!("Скрытые файлы", "Hidden files"), "F11", hidden, Action::ToggleHidden);
                        out.check(ui, tr!("Очередь операций", "Operation queue"), "Ctrl+J", queue_open, Action::ToggleQueue);
                        menu_sep(ui);
                        ui.menu_button(RichText::new(tr!("Сортировка", "Sort")).color(TEXT()), |ui| {
                            menu_style(ui, 210.0);
                            for (label, key) in [
                                (tr!("По имени", "By name"), SortKey::Name),
                                (tr!("По дате изменения", "By date modified"), SortKey::Modified),
                                (tr!("По размеру", "By size"), SortKey::Size),
                                (tr!("По типу", "By type"), SortKey::Type),
                                (tr!("По дате создания", "By date created"), SortKey::Created),
                                (tr!("По дате доступа", "By date accessed"), SortKey::Accessed),
                                (tr!("По расширению", "By extension"), SortKey::Extension),
                                (tr!("По атрибутам", "By attributes"), SortKey::Attributes),
                            ] {
                                let on = sort == key;
                                let arrow = if on { if asc { "  ↑" } else { "  ↓" } } else { "" };
                                if menu_item_icon(ui, on.then_some(Glyph::Check), &format!("{label}{arrow}"), "", true, false).clicked() {
                                    out.sort = Some(key);
                                    ui.close();
                                }
                            }
                        });
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Drive), tr!("Размер выбранных папок", "Size of selected folders"), "Alt+Enter", !is_pc, Action::CalculateSizes);
                        out.item(ui, None, tr!("Размеры всех папок", "Sizes of all folders"), "", !is_pc, Action::CalculateAllSizes);
                        out.item(ui, None, tr!("Отменить расчёт размеров", "Cancel size calculation"), "", sizing, Action::CancelSizes);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Refresh), tr!("Обновить", "Refresh"), "Ctrl+R", true, Action::Refresh);
                    });

                    top_menu(ui, tr!("Переход", "Go"), 250.0, |ui| {
                        out.item(ui, Some(Glyph::ArrowLeft), tr!("Назад", "Back"), "Alt+←", can_back, Action::Back);
                        out.item(ui, Some(Glyph::ArrowRight), tr!("Вперёд", "Forward"), "Alt+→", can_fwd, Action::Forward);
                        out.item(ui, Some(Glyph::ArrowUp), tr!("Папка выше", "Parent folder"), "Backspace", can_up, Action::GoUp);
                        out.item(ui, Some(Glyph::Pencil), tr!("Ввести путь", "Enter path"), "Ctrl+L", true, Action::EditPath);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Columns), tr!("Другая панель", "Other pane"), "Tab", dual, Action::SwitchPanel);
                        out.item(ui, Some(Glyph::Plus), tr!("Новая вкладка", "New tab"), "Ctrl+T", true, Action::NewTab);
                        out.item(ui, Some(Glyph::Close), tr!("Закрыть вкладку", "Close tab"), "Ctrl+W", tabs > 1, Action::CloseTab);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Star), tr!("Добавить в избранное", "Add to favorites"), "Ctrl+D", true, Action::AddFavorite);
                    });

                    top_menu(ui, tr!("Сервис", "Tools"), 280.0, |ui| {
                        out.item(ui, Some(Glyph::Cloud), tr!("Подключение к серверу (SFTP, FTP)…", "Connect to a server (SFTP, FTP)…"), "Ctrl+K", true, Action::Connect);
                        out.item(ui, None, tr!("Отключиться от сервера", "Disconnect from server"), "", on_server, Action::Disconnect);
                        menu_sep(ui);
                        if on_sftp {
                            out.item(ui, Some(Glyph::Terminal), tr!("SSH-терминал (PuTTY / ssh)", "SSH terminal (PuTTY / ssh)"), "Ctrl+Shift+P", true, Action::OpenTerminal);
                        } else {
                            out.item(ui, None, tr!("Открыть PowerShell / Terminal", "Open PowerShell / Terminal"), "Ctrl+Shift+P", !is_pc && !on_server, Action::OpenTerminal);
                        }
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Search), tr!("Найти файлы…", "Find files…"), "Alt+F7", true, Action::FindFiles);
                        out.item(ui, Some(Glyph::Download), tr!("Очередь операций", "Operation queue"), "Ctrl+J", true, Action::ToggleQueue);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Gear), tr!("Настройки…", "Settings…"), "", true, Action::Settings);
                    });

                    top_menu(ui, tr!("Справка", "Help"), 230.0, |ui| {
                        out.item(ui, Some(Glyph::Question), tr!("Горячие клавиши", "Keyboard shortcuts"), "F1", true, Action::Help);
                        menu_sep(ui);
                        out.item(ui, Some(Glyph::Download), tr!("Проверить обновления…", "Check for updates…"), "", true, Action::CheckUpdates);
                        out.item(ui, None, tr!("О программе Quokka", "About Quokka"), "", true, Action::About);
                    });
                    });
                });
            });

        if let Some(key) = out.sort {
            self.ap().set_sort(key);
        }
        if out.toggle_regex || out.toggle_case {
            if out.toggle_regex {
                self.settings.regex = !self.settings.regex;
            }
            if out.toggle_case {
                self.settings.case_sensitive = !self.settings.case_sensitive;
            }
            self.sync_settings();
            self.save_settings();
        }
        self.actions.extend(out.actions);
    }

    /// «О программе»: hero with the logo and version, key features and the change log.
    pub(super) fn about_ui(&mut self, ctx: &egui::Context, esc: bool) -> bool {
        let mut close = esc;
        let logo = self.logo.as_ref().map(|t| t.id());
        let current = current_release();
        let log_h = (ctx.content_rect().height() - 580.0).clamp(140.0, 320.0);
        flat_dialog(ctx, "about", ABOUT_W, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            hero(ui, logo);
            egui::Frame::NONE.inner_margin(Margin { left: PAD as i8, right: PAD as i8, top: 20, bottom: 0 }).show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            tr!(
                                "Быстрый файловый менеджер с двумя панелями и вкладками. Работает с клавиатуры \
                                 и мышью, а долгие операции выполняет в фоне — окно не замирает.",
                                "A fast file manager with two panes and tabs. Works with the keyboard \
                                 and the mouse, and runs long operations in the background — the window never freezes."
                            ),
                        )
                        .size(14.0)
                        .color(TEXT_SOFT()),
                    )
                    .wrap(),
                );
                ui.add_space(22.0);

                if let Some(rel) = current {
                    egui::Frame::NONE
                        .fill(INPUT_BG())
                        .stroke(Stroke::new(1.0, BORDER()))
                        .corner_radius(rad(12.0))
                        .inner_margin(Margin { left: 16, right: 8, top: 12, bottom: 12 })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .id_salt("about-changes")
                                .max_height(log_h)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width() - 8.0);
                                    release_ui(ui, rel);
                                });
                        });
                }
            });
            ui.add_space(18.0);
            let (line, _) = ui.allocate_exact_size(vec2(ABOUT_W, 1.0), Sense::hover());
            ui.painter().hline(line.x_range(), line.center().y, Stroke::new(1.0, BORDER()));
            egui::Frame::NONE.inner_margin(Margin::symmetric(PAD as i8, 12)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Rust · egui · Windows").size(12.5).color(MUTED()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if btn(ui, tr!("Закрыть", "Close"), BLUE(), Color32::WHITE).clicked() {
                            close = true;
                        }
                    });
                });
            });
        });
        !close
    }
}

const ABOUT_W: f32 = 640.0;
const PAD: f32 = 28.0;
const HERO_H: f32 = 196.0;

/// Tinted band with soft circles, the logo on a white disc, the name and a version pill.
fn hero(ui: &mut Ui, logo: Option<egui::TextureId>) {
    let (r, _) = ui.allocate_exact_size(vec2(ABOUT_W, HERO_H), Sense::hover());
    let p = ui.painter().with_clip_rect(r);
    p.rect_filled(r, CornerRadius { nw: rad_u8(FLAT_R as u8), ne: rad_u8(FLAT_R as u8), ..Default::default() }, BLUE_SOFT());
    let soft = |c: Color32, k: f32| mix(c, BLUE_SOFT(), k);
    p.circle_filled(pos2(r.left() + 40.0, r.top() + 10.0), 90.0, soft(BLUE(), 0.88));
    p.circle_filled(pos2(r.right() - 30.0, r.bottom() - 6.0), 110.0, soft(PURPLE, 0.9));
    p.circle_filled(pos2(r.right() - 150.0, r.top() + 26.0), 26.0, soft(TEAL, 0.86));
    p.circle_filled(pos2(r.left() + 150.0, r.bottom() - 24.0), 14.0, soft(ORANGE, 0.8));
    p.hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, BORDER()));

    let c = pos2(r.center().x, r.top() + 64.0);
    p.circle_filled(c + vec2(0.0, 3.0), 46.0, Color32::from_rgba_unmultiplied(22, 60, 140, 22));
    p.circle(c, 46.0, CARD(), Stroke::new(1.0, BORDER()));
    if let Some(id) = logo {
        let img = Rect::from_center_size(c, vec2(66.0, 66.0));
        p.image(id, img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    p.text(pos2(r.center().x, r.top() + 134.0), Align2::CENTER_CENTER, "Quokka", semibold(26.0), TEXT());

    let ver = trf!("Версия {}", "Version {}", env!("CARGO_PKG_VERSION"));
    let tag = tr!("Двухпанельный файловый менеджер", "Dual-pane file manager");
    let vg = p.layout_no_wrap(ver, semibold(12.5), BLUE());
    let tg = p.layout_no_wrap(tag.into(), regular(13.0), TEXT_SOFT());
    let pill_w = vg.size().x + 20.0;
    let total = pill_w + 10.0 + tg.size().x;
    let y = r.top() + 168.0;
    let pill = Rect::from_min_size(pos2(r.center().x - total / 2.0, y - 11.0), vec2(pill_w, 22.0));
    p.rect(pill, rad(11.0), CARD(), Stroke::new(1.0, mix(BLUE(), CARD(), 0.6)), eframe::egui::StrokeKind::Middle);
    p.galley(pill.center() - vg.size() / 2.0, vg, BLUE());
    p.galley(pos2(pill.right() + 10.0, y - tg.size().y / 2.0), tg, TEXT_SOFT());
}

fn release_ui(ui: &mut Ui, rel: &Release) {
    let note = match &rel.date {
        Some(date) => format!("Version {} · released {date}", rel.version),
        None => format!("Version {}", rel.version),
    };
    ui.label(RichText::new(note).size(12.5).color(MUTED()));
    ui.add_space(10.0);
    for (si, (name, items)) in rel.sections.iter().enumerate() {
        if si > 0 {
            ui.add_space(8.0);
        }
        let col = section_color(name);
        let g = ui.painter().layout_no_wrap(name.clone(), semibold(12.0), col);
        let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 18.0, 22.0), Sense::hover());
        let fg = mix(col, Color32::BLACK, 0.15);
        ui.painter().rect_filled(r, rad(11.0), mix(col, CARD(), 0.86));
        ui.painter().galley(r.center() - g.size() / 2.0, g, fg);
        ui.add_space(8.0);
        for item in items {
            ui.horizontal_top(|ui| {
                let (d, _) = ui.allocate_exact_size(vec2(16.0, 19.0), Sense::hover());
                ui.painter().circle_filled(pos2(d.left() + 5.0, d.center().y), 3.0, col);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    match split_title(item) {
                        Some((title, rest)) => {
                            ui.add(egui::Label::new(RichText::new(title).font(semibold(13.5)).color(TEXT())).wrap());
                            if !rest.is_empty() {
                                ui.add(egui::Label::new(RichText::new(rest).size(12.5).color(TEXT_SOFT())).wrap());
                            }
                        }
                        None => {
                            ui.add(egui::Label::new(RichText::new(item.as_str()).size(13.0).color(TEXT())).wrap());
                        }
                    }
                });
            });
            ui.add_space(10.0);
        }
    }
}

fn section_color(name: &str) -> Color32 {
    match name {
        "Added" => GREEN,
        "Changed" => BLUE(),
        "Fixed" => ORANGE,
        "Removed" => RED,
        _ => SLATE(),
    }
}

/// Splits a change into a short headline and the details: up to the first `: `, `. ` or ` (`.
/// A long first clause stays one plain paragraph.
fn split_title(s: &str) -> Option<(String, String)> {
    let (i, sep) = [": ", ". ", " ("].iter().filter_map(|sep| s.find(sep).map(|i| (i, *sep))).min_by_key(|(i, _)| *i)?;
    let title = &s[..i];
    if title.chars().count() > 60 {
        return None;
    }
    let rest = if sep == " (" { &s[i + 1..] } else { &s[i + sep.len()..] };
    Some((title.to_string(), rest.trim().to_string()))
}

struct Release {
    version: String,
    date: Option<String>,
    sections: Vec<(String, Vec<String>)>,
}

/// CHANGELOG.md parsed once: `## [version] — date`, `### Section`, `- item`.
fn releases() -> &'static [Release] {
    static CACHE: std::sync::OnceLock<Vec<Release>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        let mut out: Vec<Release> = Vec::new();
        for line in CHANGELOG.lines().map(str::trim_end) {
            if let Some(h) = line.strip_prefix("## ") {
                let h = h.replace(['[', ']'], "");
                let (v, date) = match h.split_once(" — ") {
                    Some((v, d)) => (v.trim().to_string(), Some(format_date(d.trim()))),
                    None => (h.trim().to_string(), None),
                };
                let version = if v.eq_ignore_ascii_case("unreleased") { "In development".to_string() } else { v };
                out.push(Release { version, date, sections: Vec::new() });
            } else if let Some(rel) = out.last_mut() {
                if let Some(name) = line.strip_prefix("### ") {
                    rel.sections.push((name.trim().to_string(), Vec::new()));
                } else if let Some(item) = line.strip_prefix("- ") {
                    if let Some((_, items)) = rel.sections.last_mut() {
                        items.push(plain(item));
                    }
                }
            }
        }
        // an empty «Unreleased» right after a release has nothing to show
        out.retain(|r| !r.sections.is_empty());
        out
    })
}

/// The changes of the running version; if it is missing from the changelog, the latest release.
/// Unreleased work is not shown to users.
fn current_release() -> Option<&'static Release> {
    let released = || releases().iter().filter(|r| r.date.is_some());
    released().find(|r| r.version == env!("CARGO_PKG_VERSION")).or_else(|| released().next())
}

/// Dates stay in ISO form (`2026-11-20`), as in the changelog.
fn format_date(d: &str) -> String {
    d.to_string()
}

/// Drops Markdown emphasis and code marks, which the dialog shows as plain text.
fn plain(s: &str) -> String {
    s.replace("**", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_is_parsed_into_releases() {
        let rels = releases();
        assert!(rels.len() >= 2);
        assert!(rels.iter().all(|r| !r.sections.is_empty()));
        assert!(rels.iter().any(|r| r.date.is_some()));
    }

    #[test]
    fn about_shows_the_running_version() {
        assert_eq!(current_release().unwrap().version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn headline_is_split_off() {
        assert_eq!(split_title("File search (Alt+F7): masks").unwrap(), ("File search".into(), "(Alt+F7): masks".into()));
        assert_eq!(split_title("Operation queue: progress").unwrap(), ("Operation queue".into(), "progress".into()));
        assert!(split_title("no separators").is_none());
    }
}
