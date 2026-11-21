//! Main window: top bar, icon sidebar, two tabbed panes, F1–F12 key bar, dialogs.

use crate::config::{self, Place, Settings};
use crate::fsops;
use crate::queue::{OpKind, OpSpec, Queue, State, Task};
use crate::icons::{self, Glyph};
use crate::iconsrc;
use crate::logo;
use crate::panel::{list_drives, parent_of, root_label, Panel, SortKey};
use crate::theme::{self, *};
use crate::util::*;
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, Id, Key, Layout, Margin, Modifiers, Rect, Response, RichText, Rounding,
    Sense, Stroke, Ui,
};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

mod drag;
mod find;

const ROW_H: f32 = 38.0;
const TABS_H: f32 = 46.0;
const TOOL_H: f32 = 54.0;
const HEAD_H: f32 = 36.0;
const FOOT_H: f32 = 40.0;

const SEL_ACTIVE: Color32 = Color32::from_rgb(214, 231, 255);
const SEL_INACTIVE: Color32 = Color32::from_rgb(236, 240, 246);
const MARKED_BG: Color32 = Color32::from_rgb(255, 244, 214);
const MARKED_FG: Color32 = Color32::from_rgb(196, 118, 0);
const HEADER_BG: Color32 = Color32::from_rgb(243, 247, 252);
const ROW_LINE: Color32 = Color32::from_rgb(238, 241, 246);
const TAB_ACTIVE: Color32 = Color32::from_rgb(219, 234, 254);

/// Brief launch acknowledgement; the OS does not report when the external app is ready.
#[derive(Clone)]
struct OpenFeedback {
    path: PathBuf,
    started: f64,
}

const OPEN_FEEDBACK_SECS: f64 = 1.2;

fn open_feedback_id() -> Id {
    Id::new("file-open-feedback")
}

/// egui counts double clicks globally, without checking the clicked row.
#[derive(Clone, Default)]
struct RowClicks {
    previous: Option<Id>,
    current: Option<Id>,
}

fn begin_row_clicks(ctx: &egui::Context) {
    if ctx.input(|i| i.pointer.any_click() || i.pointer.is_decidedly_dragging()) {
        ctx.data_mut(|d| {
            let clicks = d.get_temp_mut_or_default::<RowClicks>(Id::new("row-clicks"));
            clicks.previous = clicks.current.take();
        });
    }
}

fn row_double_clicked(resp: &Response, target: Id) -> bool {
    if !resp.clicked() {
        return false;
    }
    let plain = resp.ctx.input(|i| !i.modifiers.any());
    let double = resp.double_clicked();
    resp.ctx.data_mut(|d| {
        let clicks = d.get_temp_mut_or_default::<RowClicks>(Id::new("row-clicks"));
        let same = clicks.previous == Some(target);
        // Modified clicks select files and must also break the opening sequence.
        clicks.current = plain.then_some(target);
        plain && same && double
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Help,
    Rename,
    View,
    Edit,
    EditBuiltin,
    Copy,
    Move,
    ClipboardCopy,
    ClipboardCut,
    ClipboardPaste,
    MkDir,
    Delete,
    DeletePermanent,
    Zip,
    Unzip,
    ToggleQueue,
    AddFavorite,
    NewFile,
    Quit,
    ToggleHidden,
    ToggleDual,
    Settings,
    Refresh,
    Open,
    SwitchPanel,
    FocusSearch,
    ClearSearch,
    GoTo(PathBuf),
    Back,
    Forward,
    GoUp,
    Cursor(isize),
    CursorFirst,
    CursorLast,
    Mark,
    MarkMove,
    MarkAll,
    ClearMarks,
    NewTab,
    CloseTab,
    EditPath,
    OpenInExplorer,
    AddCurrentFavorite,
    UndoDelete,
    FindFiles,
    CalculateSizes,
    CalculateAllSizes,
    CancelSizes,
}

fn fkeys() -> [(Key, &'static str, Action); 12] {
    [
        (Key::F1, "Справка", Action::Help),
        (Key::F2, "Имя", Action::Rename),
        (Key::F3, "Просмотр", Action::View),
        (Key::F4, "Правка", Action::Edit),
        (Key::F5, "Копия", Action::Copy),
        (Key::F6, "Перенос", Action::Move),
        (Key::F7, "Папка", Action::MkDir),
        (Key::F8, "Удалить", Action::Delete),
        (Key::F9, "Файл", Action::NewFile),
        (Key::F10, "Выход", Action::Quit),
        (Key::F11, "Скрытые", Action::ToggleHidden),
        (Key::F12, "Панели", Action::ToggleDual),
    ]
}

fn fkey_color(i: usize) -> Color32 {
    match i {
        4 => BLUE,
        5 => GREEN,
        7 => RED,
        9 => ORANGE,
        _ => SLATE,
    }
}

enum InputKind {
    Zip(Vec<PathBuf>),
    Rename(PathBuf),
    MkDir,
    NewFile,
}

enum Modal {
    None,
    Help,
    Settings { group: SettingsGroup },
    PlaceEdit(Box<PlaceEditor>),
    Input { kind: InputKind, title: String, label: String, text: String, first: bool, error: Option<String> },
    Transfer { mv: bool, sources: Vec<PathBuf>, dest: String, overwrite: bool, first: bool },
    ConfirmDelete { sources: Vec<PathBuf>, permanent: bool },
    Viewer { path: PathBuf, content: String, info: String },
    Editor { path: PathBuf, text: String, original: String, confirm_close: bool },
    Message { title: String, text: String },
    Unzip { archives: Vec<PathBuf>, dest: String, subfolder: bool, overwrite: bool, first: bool },
    ConfirmQuit,
    Find(Box<find::FindDialog>),
}

struct Toast {
    text: String,
    error: bool,
    /// Show an «Отменить» button that restores the last recycled items.
    undo: bool,
    born: Option<f64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsGroup {
    View,
    Search,
    Sidebar,
}

impl SettingsGroup {
    const ALL: [(SettingsGroup, &'static str, Glyph); 3] = [
        (SettingsGroup::View, "Вид и файлы", Glyph::Eye),
        (SettingsGroup::Search, "Поиск", Glyph::Search),
        (SettingsGroup::Sidebar, "Избранное и меню", Glyph::Star),
    ];
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IconTab {
    Builtin,
    Windows,
    File,
}

/// State of the "add / edit favourite" dialog.
struct PlaceEditor {
    index: Option<usize>,
    name: String,
    path: String,
    glyph: Glyph,
    icon: Option<String>,
    color: Option<[u8; 3]>,
    tab: IconTab,
    /// Resource file whose icons are listed in the grid (system dll or user's exe/dll).
    lib: PathBuf,
    lib_count: Option<usize>,
    page: usize,
    first: bool,
    error: Option<String>,
    from_settings: bool,
}

impl PlaceEditor {
    fn new(index: Option<usize>, place: Place, from_settings: bool) -> Box<PlaceEditor> {
        let (tab, lib) = match place.icon.as_deref() {
            None => (IconTab::Builtin, iconsrc::system_lib("imageres.dll")),
            Some(iconsrc::SHELL) => (IconTab::Windows, iconsrc::system_lib("imageres.dll")),
            Some(spec) => {
                let (file, _) = iconsrc::split_spec(spec);
                let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_default();
                if !sys.as_os_str().is_empty() && file.starts_with(&sys) {
                    (IconTab::Windows, file)
                } else if iconsrc::is_resource_file(&file) {
                    (IconTab::File, file)
                } else {
                    (IconTab::File, iconsrc::system_lib("imageres.dll"))
                }
            }
        };
        Box::new(PlaceEditor {
            index,
            name: place.name,
            path: place.path.display().to_string(),
            glyph: place.glyph,
            icon: place.icon,
            color: place.color,
            tab,
            lib,
            lib_count: None,
            page: 0,
            first: true,
            error: None,
            from_settings,
        })
    }
}

/// Colours offered for built-in favourite icons (`None` = theme default).
const ICON_COLORS: [(Option<[u8; 3]>, &str); 12] = [
    (None, "По умолчанию"),
    (Some([22, 119, 255]), "Синий"),
    (Some([38, 190, 160]), "Бирюзовый"),
    (Some([52, 186, 120]), "Зелёный"),
    (Some([247, 182, 58]), "Жёлтый"),
    (Some([255, 150, 70]), "Оранжевый"),
    (Some([240, 90, 90]), "Красный"),
    (Some([236, 72, 153]), "Розовый"),
    (Some([139, 108, 240]), "Фиолетовый"),
    (Some([146, 98, 57]), "Коричневый"),
    (Some([100, 116, 139]), "Серый"),
    (Some([33, 39, 55]), "Чёрный"),
];

/// How a sidebar entry is drawn.
#[derive(Clone, Copy)]
struct Look {
    glyph: Glyph,
    tex: Option<egui::TextureId>,
    tint: Option<Color32>,
}

impl Look {
    fn glyph(g: Glyph) -> Look {
        Look { glyph: g, tex: None, tint: None }
    }
}

type IconCache = std::collections::HashMap<String, Option<egui::TextureHandle>>;

/// Texture for an icon spec, loaded once and cached (failures are cached too).
fn icon_texture(cache: &mut IconCache, ctx: &egui::Context, spec: &str, folder: &std::path::Path, size: u32) -> Option<egui::TextureId> {
    let key = if spec == iconsrc::SHELL { format!("shell|{}|{size}", folder.display()) } else { format!("{spec}|{size}") };
    cache
        .entry(key.clone())
        .or_insert_with(|| {
            iconsrc::load(spec, folder, size).ok().map(|img| {
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.size, img.size], &img.pixels);
                ctx.load_texture(key, ci, egui::TextureOptions::LINEAR)
            })
        })
        .as_ref()
        .map(|t| t.id())
}

fn place_look(cache: &mut IconCache, ctx: &egui::Context, pl: &Place) -> Look {
    let tex = pl.icon.as_deref().and_then(|spec| icon_texture(cache, ctx, spec, &pl.path, 64));
    Look { glyph: pl.glyph, tex, tint: pl.color.map(|[r, g, b]| Color32::from_rgb(r, g, b)) }
}

/// What the sidebar asks the app to do with its entries.
enum PlaceOp {
    Edit(usize),
    Add,
    AddPath(PathBuf),
    Move(usize, usize),
    Up(usize),
    Down(usize),
    Remove(usize),
}

pub struct Side {
    tabs: Vec<Panel>,
    cur: usize,
    /// Horizontal scroll of the tab strip when the tabs don't fit.
    tab_scroll: f32,
    /// Tab that was current last frame; when it changes, scroll it into view.
    last_cur: usize,
}

impl Side {
    fn new(p: Panel) -> Side {
        Side { tabs: vec![p], cur: 0, tab_scroll: 0.0, last_cur: usize::MAX }
    }
}

impl Side {
    fn panel(&self) -> &Panel {
        &self.tabs[self.cur]
    }
    fn panel_mut(&mut self) -> &mut Panel {
        &mut self.tabs[self.cur]
    }
}

#[derive(Default)]
struct PaneOut {
    clicked: bool,
    actions: Vec<Action>,
    /// Files dragged out of this pane by their name (file drag and drop).
    drag_start: Option<Vec<PathBuf>>,
    /// Where things can be dropped in this pane (see `drag::DropZone`).
    zone: drag::DropZone,
    /// A column was resized and the drag has ended: persist the widths.
    cols_changed: bool,
}

pub struct QuokkaApp {
    sides: [Side; 2],
    active: usize,
    /// Screen rects of the panes drawn last frame (`None` when a pane is hidden),
    /// so mouse side buttons can act on the pane under the pointer.
    pane_rects: [Option<Rect>; 2],
    settings: Settings,
    modal: Modal,
    queue: Queue,
    /// Expanded queue panel is shown (Ctrl+J).
    queue_open: bool,
    /// Quit was confirmed; close as soon as the running task has stopped.
    quitting: bool,
    allow_close: bool,
    toast: Option<Toast>,
    /// Items the last recycle moved to the bin; Ctrl+Z / «Отменить» brings them back.
    undo_recycle: Option<Vec<PathBuf>>,
    drives: Vec<PathBuf>,
    actions: Vec<Action>,
    search_id: Id,
    search_wanted: bool,
    logo: Option<egui::TextureHandle>,
    ctx: egui::Context,
    /// Sidebar entry being dragged to a new position.
    side_drag: Option<usize>,
    icon_cache: IconCache,
    file_icons: crate::file_icons::FileIcons,
    /// Last file search (Alt+F7): kept with its results while the dialog is closed.
    find: Option<Box<find::FindDialog>>,
    /// Reopen the search dialog once the dialog shown on top of it (viewer) closes.
    resume_find: bool,
    /// Files being dragged with the mouse inside the window.
    file_drag: Option<drag::FileDrag>,
    /// Drop targets of both panes, as drawn last.
    zones: [Option<drag::DropZone>; 2],
    /// Native window handle, for dragging files out to other apps.
    hwnd: isize,
    /// A drag to another app has ended: its mouse-up never reached egui, send one.
    release_pointer: bool,
    paste_held: bool,
}

fn path_edit_id(idx: usize) -> Id {
    Id::new(("path-edit", idx))
}

fn clipboard_actions(events: &mut Vec<egui::Event>) -> Vec<Action> {
    let mut actions = Vec::new();
    events.retain(|event| {
        let action = match event {
            egui::Event::Copy => Some(Action::ClipboardCopy),
            egui::Event::Cut => Some(Action::ClipboardCut),
            egui::Event::Paste(_) => Some(Action::ClipboardPaste),
            egui::Event::Key { key, pressed: true, modifiers, .. } if modifiers.command => match key {
                Key::C => Some(Action::ClipboardCopy),
                Key::X => Some(Action::ClipboardCut),
                Key::V => Some(Action::ClipboardPaste),
                _ => None,
            },
            _ => None,
        };
        if let Some(action) = action {
            if !actions.contains(&action) { actions.push(action); }
            false
        } else {
            true
        }
    });
    actions
}

fn clipboard_operation(files: crate::clipboard::Files, dest: &std::path::Path) -> Result<OpSpec, String> {
    if dest.as_os_str().is_empty() || !dest.is_dir() {
        return Err("Откройте папку для вставки файлов".into());
    }
    let dest = fs::canonicalize(dest).map_err(|e| format!("Не удалось открыть папку: {e}"))?;
    let mut sources = Vec::new();
    for path in files.paths {
        let source = fs::canonicalize(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if source.is_dir() && dest.starts_with(&source) {
            return Err("Нельзя вставить папку в саму себя или в её подпапку".into());
        }
        if files.cut && source.parent() == Some(dest.as_path()) { continue; }
        if !sources.contains(&source) { sources.push(source); }
    }
    if sources.is_empty() { return Err("Нет файлов для вставки в эту папку".into()); }
    Ok(if files.cut {
        OpSpec::Move { sources, dest, overwrite: false }
    } else {
        OpSpec::Copy { sources, dest, overwrite: false }
    })
}

impl QuokkaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);
        let home = config::home();
        let settings = Settings::load();
        let drives = list_drives();
        let right_start = drives.first().cloned().unwrap_or_else(|| home.clone());
        let mk = |path: PathBuf| {
            let mut p = Panel::new(path, settings.show_hidden);
            p.filter_regex = settings.regex;
            p.filter_case = settings.case_sensitive;
            p
        };
        QuokkaApp {
            sides: [Side::new(mk(start.unwrap_or_else(|| home.clone()))), Side::new(mk(right_start))],
            active: 0,
            pane_rects: [None, None],
            settings,
            modal: Modal::None,
            queue: Queue::default(),
            queue_open: false,
            quitting: false,
            allow_close: false,
            toast: None,
            undo_recycle: None,
            drives,
            actions: Vec::new(),
            search_id: Id::new("search-box"),
            search_wanted: false,
            logo: logo::texture(&cc.egui_ctx, 128),
            ctx: cc.egui_ctx.clone(),
            side_drag: None,
            icon_cache: IconCache::new(),
            file_icons: crate::file_icons::FileIcons::new(&cc.egui_ctx),
            find: None,
            resume_find: false,
            file_drag: None,
            zones: [None, None],
            hwnd: 0,
            release_pointer: false,
            paste_held: false,
        }
    }

    fn ap(&mut self) -> &mut Panel {
        self.sides[self.active].panel_mut()
    }

    fn ap_ref(&self) -> &Panel {
        self.sides[self.active].panel()
    }

    /// Default target for F5/F6-like operations: the other panel's folder, or the
    /// active one when the other shows «This PC» (or only one panel is visible).
    fn dest_dir(&self) -> PathBuf {
        let other = self.other_path();
        if other.as_os_str().is_empty() {
            self.ap_ref().path.clone()
        } else {
            other
        }
    }

    fn other_path(&self) -> PathBuf {
        self.sides[1 - self.active].panel().path.clone()
    }

    fn all_panels(&mut self) -> impl Iterator<Item = &mut Panel> {
        self.sides.iter_mut().flat_map(|s| s.tabs.iter_mut())
    }

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some(Toast { text: text.into(), error, undo: false, born: None });
    }

    fn report(&mut self, r: Result<(), String>) {
        if let Err(e) = r {
            self.toast(e, true);
        }
    }

    fn reload_all(&mut self) {
        for p in self.all_panels() {
            p.reload();
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.toast(format!("Не удалось сохранить настройки: {e}"), true);
        }
    }

    /// Push hidden-files and search options from the settings into every tab.
    fn sync_settings(&mut self) {
        let (h, re, cs) = (self.settings.show_hidden, self.settings.regex, self.settings.case_sensitive);
        for p in self.sides.iter_mut().flat_map(|s| s.tabs.iter_mut()) {
            if p.show_hidden != h {
                p.show_hidden = h;
                p.reload();
            }
            if p.filter_regex != re || p.filter_case != cs {
                p.filter_regex = re;
                p.filter_case = cs;
                p.apply_filter_sort();
            }
        }
    }

    fn place_op(&mut self, op: PlaceOp, from_settings: bool) {
        let n = self.settings.places.len();
        match op {
            PlaceOp::Edit(i) => {
                let Some(pl) = self.settings.places.get(i).cloned() else { return };
                self.modal = Modal::PlaceEdit(PlaceEditor::new(Some(i), pl, from_settings));
            }
            PlaceOp::Add => {
                let p = self.ap_ref();
                let pl = if p.is_pc() {
                    Place::new("Этот компьютер", PathBuf::new(), Glyph::Desktop)
                } else {
                    Place::new(p.title(), p.path.clone(), Glyph::Folder)
                };
                self.modal = Modal::PlaceEdit(PlaceEditor::new(None, pl, from_settings));
            }
            PlaceOp::AddPath(path) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| crate::panel::root_label(&path));
                let glyph = if path.parent().is_none() { Glyph::Drive } else { Glyph::Folder };
                self.modal = Modal::PlaceEdit(PlaceEditor::new(None, Place::new(name, path, glyph), from_settings));
            }
            PlaceOp::Move(from, to) if from < n => {
                let item = self.settings.places.remove(from);
                let to = if to > from { to - 1 } else { to }.min(self.settings.places.len());
                self.settings.places.insert(to, item);
                self.save_settings();
            }
            PlaceOp::Up(i) if i > 0 && i < n => {
                self.settings.places.swap(i, i - 1);
                self.save_settings();
            }
            PlaceOp::Down(i) if i + 1 < n => {
                self.settings.places.swap(i, i + 1);
                self.save_settings();
            }
            PlaceOp::Remove(i) if i < n => {
                self.settings.places.remove(i);
                self.save_settings();
            }
            _ => {}
        }
    }

    fn text_focus(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.focused())
            .is_some_and(|f| f == self.search_id || f == path_edit_id(0) || f == path_edit_id(1))
    }

    fn idle(&self) -> bool {
        matches!(self.modal, Modal::None)
    }

    // ───────────────────────────── input ─────────────────────────────

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let typing = self.text_focus(ctx);
        let page = ((self.ap_ref().view_height / ROW_H) as isize - 1).max(1);
        let mut acts = Vec::new();
        let mut side = None;
        ctx.input_mut(|i| {
            // egui matches modifiers loosely: `consume_key(NONE, F7)` also eats Alt+F7.
            // So combos with Alt/Shift must be consumed before the plain keys.
            let modified = [
                (Modifiers::ALT, Key::F5, Action::Zip),
                (Modifiers::ALT, Key::F6, Action::Unzip),
                (Modifiers::ALT, Key::F7, Action::FindFiles),
                (Modifiers::ALT, Key::Enter, Action::CalculateSizes),
                (Modifiers::SHIFT, Key::F4, Action::EditBuiltin),
                (Modifiers::SHIFT, Key::F8, Action::DeletePermanent),
            ];
            for (m, k, a) in modified {
                if i.consume_key(m, k) {
                    acts.push(a);
                }
            }
            for (k, _, a) in fkeys() {
                if i.consume_key(Modifiers::NONE, k) {
                    acts.push(a);
                }
            }
            let combos = [
                (Modifiers::COMMAND, Key::R, Action::Refresh),
                (Modifiers::COMMAND, Key::F, Action::FocusSearch),
                (Modifiers::COMMAND, Key::T, Action::NewTab),
                (Modifiers::COMMAND, Key::W, Action::CloseTab),
                (Modifiers::COMMAND, Key::L, Action::EditPath),
                (Modifiers::COMMAND, Key::D, Action::AddFavorite),
                (Modifiers::COMMAND, Key::Z, Action::UndoDelete),
                (Modifiers::ALT, Key::ArrowLeft, Action::Back),
                (Modifiers::ALT, Key::ArrowRight, Action::Forward),
                (Modifiers::ALT, Key::ArrowUp, Action::GoUp),
            ];
            for (m, k, a) in combos {
                if i.consume_key(m, k) {
                    acts.push(a);
                }
            }
            if i.consume_key(Modifiers::COMMAND, Key::J) {
                acts.push(Action::ToggleQueue);
            }
            if i.consume_key(Modifiers::NONE, Key::Tab) {
                acts.push(Action::SwitchPanel);
            }
            let nav = [
                (Key::ArrowUp, Action::Cursor(-1)),
                (Key::ArrowDown, Action::Cursor(1)),
                (Key::PageUp, Action::Cursor(-page)),
                (Key::PageDown, Action::Cursor(page)),
            ];
            for (k, a) in nav {
                if i.consume_key(Modifiers::NONE, k) {
                    acts.push(a);
                }
            }
            if !typing {
                acts.extend(clipboard_actions(&mut i.events));
                let keys = [
                    (Modifiers::NONE, Key::Home, Action::CursorFirst),
                    (Modifiers::NONE, Key::End, Action::CursorLast),
                    (Modifiers::NONE, Key::Enter, Action::Open),
                    (Modifiers::NONE, Key::Backspace, Action::GoUp),
                    (Modifiers::NONE, Key::Insert, Action::MarkMove),
                    (Modifiers::NONE, Key::Space, Action::Mark),
                    (Modifiers::COMMAND, Key::A, Action::MarkAll),
                    (Modifiers::NONE, Key::Escape, Action::ClearMarks),
                    (Modifiers::SHIFT, Key::Delete, Action::DeletePermanent),
                    (Modifiers::NONE, Key::Delete, Action::Delete),
                ];
                for (m, k, a) in keys {
                    if i.consume_key(m, k) {
                        acts.push(a);
                    }
                }
            } else if i.key_pressed(Key::Escape) {
                acts.push(Action::ClearSearch);
            }
            // mouse side buttons: X1 = back, X2 = forward
            let back = i.pointer.button_pressed(egui::PointerButton::Extra1);
            let fwd = i.pointer.button_pressed(egui::PointerButton::Extra2);
            if back || fwd {
                // act on the pane under the pointer, if any; otherwise on the active one
                if let Some(pos) = i.pointer.hover_pos() {
                    if let Some(p) = self.pane_rects.iter().position(|r| r.is_some_and(|r| r.contains(pos))) {
                        side = Some(p);
                    }
                }
                acts.push(if back { Action::Back } else { Action::Forward });
            }
        });
        if let Some(p) = side {
            self.active = p;
        }
        self.actions.extend(acts);
    }

    fn focus_policy(&mut self, ctx: &egui::Context) {
        if self.idle() {
            if let Some(f) = ctx.memory(|m| m.focused()) {
                let editing = |i: usize| self.sides[i].panel().editing_path.is_some();
                let ok = (f == self.search_id && self.search_wanted)
                    || (f == path_edit_id(0) && editing(0))
                    || (f == path_edit_id(1) && editing(1));
                if !ok {
                    ctx.memory_mut(|m| m.surrender_focus(f));
                }
            }
        }
        if !ctx.memory(|m| m.has_focus(self.search_id)) {
            self.search_wanted = false;
        }
    }

    // ───────────────────────────── actions ─────────────────────────────

    fn run(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::CalculateSizes | Action::CalculateAllSizes => {
                let count = self.ap().calculate_sizes(a == Action::CalculateAllSizes, ctx.clone());
                if count == 0 { self.toast("Выберите папку для расчёта размера", false); }
            }
            Action::CancelSizes => self.ap().folder_sizes.stop(),
            Action::Help => self.modal = Modal::Help,
            Action::FindFiles => self.open_find(),
            Action::UndoDelete => match self.undo_recycle.take() {
                Some(originals) => {
                    self.toast = None;
                    self.enqueue(OpSpec::Restore { originals });
                }
                None => self.toast("Нечего отменять", false),
            },
            Action::Settings => self.modal = Modal::Settings { group: SettingsGroup::View },
            Action::SwitchPanel => self.active = 1 - self.active,
            Action::Cursor(d) => self.ap().move_cursor(d),
            Action::CursorFirst => self.ap().set_cursor(0),
            Action::CursorLast => self.ap().set_cursor(usize::MAX),
            Action::Mark => {
                let p = self.ap();
                let c = p.cursor;
                p.toggle_mark(c);
            }
            Action::MarkMove => {
                let p = self.ap();
                let c = p.cursor;
                p.toggle_mark(c);
                p.move_cursor(1);
            }
            Action::MarkAll => self.ap().mark_all(),
            Action::ClearMarks => self.ap().marked.clear(),
            Action::Open => self.open_current(),
            Action::GoUp => {
                let r = self.ap().go_up();
                self.report(r);
            }
            Action::Back => {
                let r = self.ap().go_back();
                self.report(r);
            }
            Action::Forward => {
                let r = self.ap().go_forward();
                self.report(r);
            }
            Action::GoTo(path) => {
                let r = self.ap().navigate(path);
                self.report(r);
            }
            Action::Refresh => {
                self.drives = list_drives();
                self.reload_all();
            }
            Action::ToggleHidden => {
                self.settings.show_hidden = !self.settings.show_hidden;
                self.sync_settings();
                self.save_settings();
                let h = self.settings.show_hidden;
                self.toast(if h { "Скрытые файлы показаны" } else { "Скрытые файлы не показываются" }, false);
            }
            Action::ToggleDual => {
                self.settings.dual = !self.settings.dual;
                self.save_settings();
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::FocusSearch => {
                self.search_wanted = true;
                ctx.memory_mut(|m| m.request_focus(self.search_id));
            }
            Action::ClearSearch => {
                let p = self.ap();
                p.filter.clear();
                p.apply_filter_sort();
                ctx.memory_mut(|m| m.surrender_focus(self.search_id));
            }
            Action::NewTab => {
                let side = &mut self.sides[self.active];
                let dup = side.panel().duplicate();
                side.tabs.insert(side.cur + 1, dup);
                side.cur += 1;
            }
            Action::CloseTab => {
                let side = &mut self.sides[self.active];
                if side.tabs.len() > 1 {
                    side.tabs.remove(side.cur);
                    side.cur = side.cur.min(side.tabs.len() - 1);
                }
            }
            Action::AddCurrentFavorite => self.place_op(PlaceOp::Add, false),
            Action::OpenInExplorer => {
                let path = self.ap_ref().path.clone();
                if path.as_os_str().is_empty() {
                    return;
                }
                if let Err(e) = fsops::open_external(&path) {
                    self.toast(format!("Не удалось открыть Проводник: {e}"), true);
                }
            }
            Action::EditPath => {
                let p = self.ap();
                p.editing_path = Some(p.path.display().to_string());
                p.focus_path_edit = true;
            }
            Action::Rename => {
                let Some(e) = self.ap_ref().current().cloned() else { return };
                if e.is_parent || e.is_drive {
                    return;
                }
                self.modal = Modal::Input {
                    kind: InputKind::Rename(e.path.clone()),
                    title: "Переименование".into(),
                    label: format!("Новое имя для «{}»", e.name),
                    text: e.name.clone(),
                    first: true,
                    error: None,
                };
            }
            Action::AddFavorite => {
                // folder under the cursor, otherwise the folder that is open
                let p = self.ap_ref();
                let path = match p.current() {
                    Some(e) if e.is_dir && !e.is_parent => e.path.clone(),
                    _ => p.path.clone(),
                };
                if path.as_os_str().is_empty() {
                    self.place_op(PlaceOp::Add, false);
                } else {
                    self.place_op(PlaceOp::AddPath(path), false);
                }
            }
            Action::Zip => {
                let sources = self.ap_ref().targets();
                if sources.is_empty() {
                    self.toast("Ничего не выбрано", true);
                    return;
                }
                let base = if sources.len() == 1 {
                    let p = &sources[0];
                    if p.is_dir() {
                        p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    } else {
                        p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    }
                } else {
                    self.ap_ref().title()
                };
                let base = if base.is_empty() { "Архив".to_string() } else { base };
                let name = format!("{base}.zip");
                // like F5: the archive goes to the other panel
                let dir = self.dest_dir();
                let text = if dir.as_os_str().is_empty() { name } else { dir.join(name).display().to_string() };
                let label = format!("Будет упаковано: {}", items_text(sources.len() as u64));
                self.modal = Modal::Input {
                    kind: InputKind::Zip(sources),
                    title: "Создать ZIP-архив".into(),
                    label,
                    text,
                    first: true,
                    error: None,
                };
            }
            Action::Unzip => {
                let archives: Vec<PathBuf> = self.ap_ref().targets().into_iter().filter(|p| is_zip(p)).collect();
                if archives.is_empty() {
                    self.toast("Выберите ZIP-архив", true);
                    return;
                }
                // like F5: unpack into the other panel
                let dest = self.dest_dir().display().to_string();
                self.modal = Modal::Unzip { archives, dest, subfolder: true, overwrite: false, first: true };
            }
            Action::ToggleQueue => self.queue_open = !self.queue_open,
            Action::MkDir | Action::NewFile => {
                if self.ap_ref().is_pc() {
                    self.toast("Сначала откройте папку", true);
                    return;
                }
                let dir = a == Action::MkDir;
                self.modal = Modal::Input {
                    kind: if dir { InputKind::MkDir } else { InputKind::NewFile },
                    title: if dir { "Новая папка" } else { "Новый файл" }.into(),
                    label: format!("Создать в {}", self.ap_ref().path.display()),
                    text: if dir { "Новая папка" } else { "Новый файл.txt" }.into(),
                    first: true,
                    error: None,
                };
            }
            Action::View => {
                let Some(e) = self.ap_ref().current().cloned() else { return };
                if e.is_dir {
                    self.open_current();
                } else {
                    self.modal = load_viewer(e.path);
                }
            }
            Action::Edit | Action::EditBuiltin => {
                let Some(e) = self.ap_ref().current().cloned() else { return };
                if e.is_dir || e.is_drive {
                    self.toast("Папку нельзя редактировать", true);
                } else if a == Action::EditBuiltin {
                    self.modal = load_editor(e.path);
                } else if let Err(err) = fsops::open_in_editor(&self.settings.editor, &e.path) {
                    self.toast(format!("Не удалось запустить редактор: {err}. Встроенный — Shift+F4"), true);
                }
            }
            Action::Copy | Action::Move => {
                let sources = self.ap_ref().targets();
                if sources.is_empty() {
                    self.toast("Ничего не выбрано", true);
                    return;
                }
                let other = self.other_path();
                let dest = if other.as_os_str().is_empty() { String::new() } else { other.display().to_string() };
                self.modal = Modal::Transfer { mv: a == Action::Move, sources, dest, overwrite: false, first: true };
            }
            Action::ClipboardCopy | Action::ClipboardCut => {
                let paths = self.ap_ref().targets();
                if paths.is_empty() {
                    self.toast("Ничего не выбрано", true);
                    return;
                }
                let cut = a == Action::ClipboardCut;
                let count = paths.len();
                match crate::clipboard::write(self.hwnd, &crate::clipboard::Files { paths, cut }) {
                    Ok(()) => self.toast(format!("{}: {}. Перейдите в папку и нажмите Ctrl+V",
                        if cut { "Вырезано" } else { "В буфере обмена" }, items_text(count as u64)), false),
                    Err(err) => self.toast(err, true),
                }
            }
            Action::ClipboardPaste => {
                let result = crate::clipboard::read(self.hwnd)
                    .and_then(|files| clipboard_operation(files, &self.ap_ref().path));
                match result {
                    Ok(spec) => {
                        let (sources, dest, mv) = match &spec {
                            OpSpec::Copy { sources, dest, .. } => (sources, dest, false),
                            OpSpec::Move { sources, dest, .. } => (sources, dest, true),
                            _ => unreachable!(),
                        };
                        let conflicts = sources.iter().any(|s| {
                            let target = dest.join(s.file_name().unwrap_or_default());
                            target != *s && target.exists()
                        });
                        if conflicts {
                            self.modal = Modal::Transfer { mv, sources: sources.clone(),
                                dest: dest.display().to_string(), overwrite: false, first: true };
                        } else {
                            self.enqueue(spec);
                        }
                    }
                    Err(err) => self.toast(err, true),
                }
            }
            Action::Delete | Action::DeletePermanent => {
                let sources = self.ap_ref().targets();
                if sources.is_empty() {
                    self.toast("Ничего не выбрано", true);
                    return;
                }
                let permanent = a == Action::DeletePermanent || !fsops::HAS_RECYCLE_BIN;
                self.modal = Modal::ConfirmDelete { sources, permanent };
            }
        }
    }

    fn open_current(&mut self) {
        let Some(e) = self.ap_ref().current().cloned() else { return };
        if e.is_dir {
            let r = self.ap().navigate(e.path);
            self.report(r);
        } else if let Err(err) = fsops::open_external(&e.path) {
            self.toast(format!("Не удалось открыть «{}»: {}", e.name, err), true);
        } else {
            let feedback = OpenFeedback { path: e.path, started: self.ctx.input(|i| i.time) };
            self.ctx.data_mut(|d| d.insert_temp(open_feedback_id(), feedback));
            self.ctx.request_repaint();
        }
    }

    fn apply_input(&mut self, kind: &InputKind, text: &str) -> Result<(), String> {
        let name = text.trim();
        if name.is_empty() {
            return Err("Имя не может быть пустым".into());
        }
        let dir = self.ap_ref().path.clone();
        let select;
        match kind {
            InputKind::Zip(sources) => {
                let mut target = PathBuf::from(name);
                if target.is_relative() {
                    target = dir.join(target);
                }
                if target.extension().map_or(true, |e| !e.eq_ignore_ascii_case("zip")) {
                    let mut s = target.into_os_string();
                    s.push(".zip");
                    target = PathBuf::from(s);
                }
                if target.exists() {
                    return Err(format!("«{}» уже существует", target.display()));
                }
                if target.parent().is_some_and(|p| !p.is_dir()) {
                    return Err("Папка назначения не существует".into());
                }
                self.enqueue(OpSpec::Zip { sources: sources.clone(), dest: target });
                return Ok(());
            }
            InputKind::Rename(old) => {
                if name.contains(['/', '\\']) {
                    return Err("Имя не может содержать косую черту".into());
                }
                let new = old.with_file_name(name);
                let old_name = old.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                if new.exists() && old_name != name.to_lowercase() {
                    return Err(format!("«{name}» уже существует"));
                }
                fs::rename(old, &new).map_err(|e| e.to_string())?;
                select = name.to_string();
            }
            InputKind::MkDir => {
                let p = dir.join(name);
                if p.exists() {
                    return Err(format!("«{name}» уже существует"));
                }
                fs::create_dir_all(&p).map_err(|e| e.to_string())?;
                select = name.split(['/', '\\']).next().unwrap_or(name).to_string();
            }
            InputKind::NewFile => {
                if name.contains(['/', '\\']) {
                    return Err("Имя не может содержать косую черту".into());
                }
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(dir.join(name))
                    .map_err(|e| e.to_string())?;
                select = name.to_string();
            }
        }
        self.reload_all();
        self.ap().select_name(&select);
        Ok(())
    }

    /// Add an operation to the queue; it starts right away if nothing else is running.
    fn enqueue(&mut self, spec: OpSpec) {
        let busy = self.queue.is_active();
        let title = spec.title();
        self.queue.push(spec);
        self.ap().marked.clear();
        if busy {
            self.toast(format!("Добавлено в очередь: {title}"), false);
        }
        self.ctx.request_repaint();
    }

    fn poll_queue(&mut self, ctx: &egui::Context) {
        let finished = self.queue.poll(ctx);
        if finished.is_empty() {
            return;
        }
        self.reload_all();
        for f in finished {
            let error = f.result.is_err() && !f.cancelled;
            let text = if f.cancelled { "Операция отменена".to_string() } else { f.result.unwrap_or_else(|e| e) };
            // even a failed or cancelled recycle may have moved some items already
            let gone: Vec<PathBuf> = match f.spec {
                OpSpec::Recycle { sources } => sources.into_iter().filter(|p| !p.exists()).collect(),
                _ => Vec::new(),
            };
            let undo = !gone.is_empty();
            if undo {
                self.undo_recycle = Some(gone);
            }
            self.toast = Some(Toast { text, error, undo, born: None });
        }
    }

    // ───────────────────────────── layout ─────────────────────────────

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .exact_height(66.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::none()
                    .fill(CARD)
                    .inner_margin(Margin::symmetric(16.0, 0.0))
                    .stroke(Stroke::new(1.0, BORDER)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if let Some(t) = &self.logo {
                        ui.image((t.id(), vec2(46.0, 46.0)));
                    }
                    ui.label(RichText::new("Quokka").font(semibold(21.0)).color(TEXT));
                    vsep(ui);

                    let (can_back, can_fwd, can_up, is_pc) = {
                        let p = self.sides[self.active].panel();
                        (!p.back.is_empty(), !p.forward.is_empty(), parent_of(&p.path).is_some(), p.is_pc())
                    };
                    if top_btn(ui, Glyph::ArrowLeft, TEXT_SOFT, can_back, false, true).on_hover_text("Назад (Alt+←)").clicked() {
                        self.actions.push(Action::Back);
                    }
                    if top_btn(ui, Glyph::ArrowRight, TEXT_SOFT, can_fwd, false, true).on_hover_text("Вперёд (Alt+→)").clicked() {
                        self.actions.push(Action::Forward);
                    }
                    if top_btn(ui, Glyph::ArrowUp, TEXT_SOFT, can_up, false, true).on_hover_text("Вверх (Backspace)").clicked() {
                        self.actions.push(Action::GoUp);
                    }
                    if top_btn(ui, Glyph::FolderPlus, GREEN, !is_pc, false, false).on_hover_text("Новая папка (F7)").clicked() {
                        self.actions.push(Action::MkDir);
                    }
                    vsep(ui);

                    // search
                    let right_w = 4.0 * 40.0 + 3.0 * 8.0 + 26.0;
                    let sw = (ui.available_width() - right_w - 12.0).clamp(180.0, 640.0);
                    let (sr, sresp) = ui.allocate_exact_size(vec2(sw, 42.0), Sense::click());
                    let focused = ctx.memory(|m| m.has_focus(self.search_id));
                    let search_err = self.sides[self.active].panel().filter_error.clone();
                    let painter = ui.painter().clone();
                    let border = if search_err.is_some() {
                        RED
                    } else if focused {
                        BLUE
                    } else {
                        BORDER
                    };
                    painter.rect(sr, 10.0, CARD, Stroke::new(if focused || search_err.is_some() { 1.5 } else { 1.0 }, border));
                    icons::glyph(&painter, Rect::from_center_size(pos2(sr.left() + 24.0, sr.center().y), vec2(20.0, 20.0)), Glyph::Search, TEXT_SOFT);
                    let kbd = painter.layout_no_wrap("Ctrl+F".into(), regular(12.0), TEXT_SOFT);
                    let kr = Rect::from_center_size(
                        pos2(sr.right() - 12.0 - (kbd.size().x + 14.0) / 2.0, sr.center().y),
                        vec2(kbd.size().x + 14.0, 22.0),
                    );
                    painter.rect(kr, 6.0, INPUT_BG, Stroke::new(1.0, BORDER));
                    painter.galley(kr.center() - kbd.size() / 2.0, kbd, TEXT_SOFT);

                    // regex / case toggles inside the search box
                    let mut toggles_changed = false;
                    let mut tx = kr.left() - 6.0;
                    for (k, label, tip) in [
                        (0, "Aa", "Учитывать регистр"),
                        (1, ".*", "Регулярные выражения (Alt+R)"),
                    ] {
                        let on = if k == 0 { self.settings.case_sensitive } else { self.settings.regex };
                        let br = Rect::from_min_max(pos2(tx - 28.0, sr.center().y - 13.0), pos2(tx, sr.center().y + 13.0));
                        let resp = ui.interact(br, Id::new(("search-toggle", k)), Sense::click());
                        let fill = if on {
                            BLUE_SOFT
                        } else if resp.hovered() {
                            HOVER
                        } else {
                            CARD
                        };
                        painter.rect(br, 6.0, fill, Stroke::new(1.0, if on { BLUE } else { BORDER }));
                        painter.text(br.center(), Align2::CENTER_CENTER, label, semibold(13.0), if on { BLUE } else { TEXT_SOFT });
                        if resp.on_hover_text(tip).clicked() {
                            if k == 0 {
                                self.settings.case_sensitive = !self.settings.case_sensitive;
                            } else {
                                self.settings.regex = !self.settings.regex;
                            }
                            toggles_changed = true;
                        }
                        tx = br.left() - 4.0;
                    }
                    if ctx.input_mut(|i| i.consume_key(Modifiers::ALT, Key::R)) {
                        self.settings.regex = !self.settings.regex;
                        toggles_changed = true;
                    }
                    if toggles_changed {
                        self.sync_settings();
                        self.save_settings();
                    }

                    let tr = Rect::from_min_max(pos2(sr.left() + 44.0, sr.center().y - 12.0), pos2(tx - 4.0, sr.center().y + 12.0));
                    let hint = if self.settings.regex { "Регулярное выражение, например ^отчёт.*\\.pdf$" } else { "Поиск файлов и папок..." };
                    let a = self.active;
                    let p = self.sides[a].panel_mut();
                    let te = ui.put(
                        tr,
                        egui::TextEdit::singleline(&mut p.filter)
                            .id(self.search_id)
                            .hint_text(RichText::new(hint).color(MUTED))
                            .frame(false)
                            .font(if self.settings.regex { mono(14.0) } else { regular(15.0) })
                            .text_color(TEXT),
                    );
                    if te.changed() {
                        p.apply_filter_sort();
                    }
                    if let Some(e) = &search_err {
                        sresp.clone().on_hover_text(RichText::new(e.as_str()).color(RED));
                    }
                    let pressed = ctx.input(|i| i.pointer.primary_pressed());
                    if pressed && (te.hovered() || sresp.hovered()) {
                        self.search_wanted = true;
                        ctx.memory_mut(|m| m.request_focus(self.search_id));
                    }

                    let space = ui.available_width() - (4.0 * 40.0 + 3.0 * 8.0) - 6.0;
                    if space > 0.0 {
                        ui.add_space(space);
                    }
                    let busy = self.queue.pending_count() + usize::from(self.queue.running().is_some());
                    let qr = top_btn(ui, Glyph::Download, if busy > 0 { BLUE } else { TEXT_SOFT }, true, self.queue_open, false)
                        .on_hover_text("Очередь операций (Ctrl+J)");
                    if busy > 0 {
                        let c = pos2(qr.rect.right() - 6.0, qr.rect.top() + 6.0);
                        ui.painter().circle_filled(c, 8.0, BLUE);
                        ui.painter().text(c, Align2::CENTER_CENTER, busy.to_string(), semibold(10.5), Color32::WHITE);
                    }
                    if qr.clicked() {
                        self.actions.push(Action::ToggleQueue);
                    }
                    if top_btn(ui, Glyph::Columns, if self.settings.dual { BLUE } else { TEXT_SOFT }, true, self.settings.dual, false)
                        .on_hover_text("Две панели (F12)")
                        .clicked()
                    {
                        self.actions.push(Action::ToggleDual);
                    }
                    if top_btn(ui, Glyph::Gear, TEXT_SOFT, true, false, false).on_hover_text("Настройки").clicked() {
                        self.actions.push(Action::Settings);
                    }
                    if top_btn(ui, Glyph::Question, TEXT_SOFT, true, false, false).on_hover_text("Справка (F1)").clicked() {
                        self.actions.push(Action::Help);
                    }
                });
            });
    }

    fn fkey_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("fkeys")
            .exact_height(52.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::none()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::symmetric(12.0, 8.0)),
            )
            .show(ctx, |ui| {
                let avail = ui.available_rect_before_wrap();
                let gap = 6.0;
                let w = (avail.width() - gap * 11.0) / 12.0;
                let painter = ui.painter().clone();
                for (i, (_, label, a)) in fkeys().into_iter().enumerate() {
                    let r = Rect::from_min_size(pos2(avail.left() + i as f32 * (w + gap), avail.top()), vec2(w, avail.height()));
                    let resp = ui.interact(r, Id::new(("fkey", i)), Sense::click());
                    if resp.hovered() {
                        painter.rect_filled(r, 8.0, HOVER);
                    }
                    let col = fkey_color(i);
                    let badge = Rect::from_min_size(pos2(r.left() + 4.0, r.center().y - 12.0), vec2(36.0, 24.0));
                    painter.rect_filled(badge, 6.0, mix(col, Color32::WHITE, 0.84));
                    painter.text(badge.center(), Align2::CENTER_CENTER, format!("F{}", i + 1), semibold(13.0), mix(col, Color32::BLACK, 0.1));
                    let on = match a {
                        Action::ToggleHidden => Some(self.settings.show_hidden),
                        Action::ToggleDual => Some(self.settings.dual),
                        _ => None,
                    };
                    painter.with_clip_rect(r).text(
                        pos2(badge.right() + 7.0, r.center().y),
                        Align2::LEFT_CENTER,
                        label,
                        regular(13.5),
                        TEXT,
                    );
                    if on == Some(true) {
                        painter.circle_filled(pos2(r.right() - 7.0, r.center().y), 3.0, BLUE);
                    }
                    if resp.clicked() {
                        self.actions.push(a);
                    }
                }
            });
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(if self.settings.sidebar_labels { 220.0 } else { 68.0 })
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::none()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, BORDER))
                    .inner_margin(Margin::symmetric(12.0, 14.0)),
            )
            .show(ctx, |ui| {
                let cur = self.sides[self.active].panel().path.clone();
                let labels = self.settings.sidebar_labels;
                let looks: Vec<Look> = self.settings.places.iter().map(|pl| place_look(&mut self.icon_cache, ctx, pl)).collect();
                let w = ui.available_width();
                let mut op = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = if labels { 4.0 } else { 8.0 };
                    let n = self.settings.places.len();
                    let mut rects = Vec::with_capacity(n);
                    for (i, place) in self.settings.places.iter().enumerate() {
                        let dragging_me = self.side_drag == Some(i);
                        let resp = side_btn(ui, looks[i], cur == place.path || dragging_me, labels.then_some(place.name.as_str()), w);
                        rects.push(resp.rect);
                        if resp.drag_started_by(egui::PointerButton::Primary) {
                            self.side_drag = Some(i);
                        }
                        let resp = if labels || self.side_drag.is_some() {
                            resp
                        } else {
                            resp.on_hover_text(format!("{}\nПеретащите, чтобы изменить порядок", place.name))
                        };
                        if resp.clicked() {
                            self.actions.push(Action::GoTo(place.path.clone()));
                        }
                        resp.context_menu(|ui| {
                            menu_style(ui, 240.0);
                            menu_title(ui, &place.name);
                            if menu_item(ui, "Изменить название и значок…", "", true, false).clicked() {
                                op = Some(PlaceOp::Edit(i));
                                ui.close_menu();
                            }
                            if menu_item(ui, "Переместить выше", "", i > 0, false).clicked() {
                                op = Some(PlaceOp::Up(i));
                                ui.close_menu();
                            }
                            if menu_item(ui, "Переместить ниже", "", i + 1 < n, false).clicked() {
                                op = Some(PlaceOp::Down(i));
                                ui.close_menu();
                            }
                            menu_sep(ui);
                            if menu_item(ui, "Убрать из избранного", "", true, true).clicked() {
                                op = Some(PlaceOp::Remove(i));
                                ui.close_menu();
                            }
                        });
                    }
                    // drop indicator and floating copy of the dragged entry
                    if let Some(from) = self.side_drag.filter(|&f| f < n) {
                        let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
                        let target = pos.map_or(from, |p| rects.iter().position(|r| p.y < r.center().y).unwrap_or(n));
                        if down {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            if target != from && target != from + 1 {
                                let y = if target < n { rects[target].top() - 3.0 } else { rects[n - 1].bottom() + 3.0 };
                                let (x0, x1) = (rects[0].left() - 2.0, rects[0].left() + w + 2.0);
                                ui.painter().line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(3.0, BLUE));
                            }
                            if let Some(p) = pos {
                                let layer = egui::LayerId::new(egui::Order::Tooltip, Id::new("side-ghost"));
                                let gp = ui.ctx().layer_painter(layer);
                                let place = &self.settings.places[from];
                                let galley = gp.layout_no_wrap(place.name.clone(), regular(13.5), TEXT);
                                let gr = Rect::from_min_size(p + vec2(14.0, -16.0), vec2(galley.size().x + 46.0, 32.0));
                                gp.rect(gr, 8.0, CARD, Stroke::new(1.0, BLUE));
                                draw_look(&gp, Rect::from_center_size(pos2(gr.left() + 18.0, gr.center().y), vec2(18.0, 18.0)), looks[from], BLUE);
                                gp.galley(pos2(gr.left() + 34.0, gr.center().y - galley.size().y / 2.0), galley, TEXT);
                            }
                        } else {
                            if target != from && target != from + 1 {
                                op = Some(PlaceOp::Move(from, target));
                            }
                            self.side_drag = None;
                        }
                    } else {
                        self.side_drag = None;
                    }
                    let add = side_btn(ui, Look::glyph(Glyph::Plus), false, labels.then_some("Добавить в избранное"), w);
                    let add = if labels { add } else { add.on_hover_text("Добавить текущую папку в избранное") };
                    if add.clicked() {
                        op = Some(PlaceOp::Add);
                    }
                    if self.settings.show_drives {
                        let (r, _) = ui.allocate_exact_size(vec2(w, 10.0), Sense::hover());
                        ui.painter().line_segment([pos2(r.left() + 6.0, r.center().y), pos2(r.right() - 6.0, r.center().y)], Stroke::new(1.0, BORDER));
                        for d in &self.drives {
                            let title = format!("Локальный диск ({})", root_label(d));
                            let tip = match (fs2::available_space(d), fs2::total_space(d)) {
                                (Ok(f), Ok(t)) => format!("{title}\nСвободно {} из {}", format_size(f), format_size(t)),
                                _ => title.clone(),
                            };
                            let resp = side_btn(ui, Look::glyph(Glyph::Drive), cur == *d, labels.then_some(title.as_str()), w);
                            if !labels {
                                ui.painter().text(
                                    resp.rect.right_bottom() - vec2(5.0, 4.0),
                                    Align2::RIGHT_BOTTOM,
                                    root_label(d).trim_end_matches(':'),
                                    semibold(10.0),
                                    if cur == *d { BLUE } else { TEXT_SOFT },
                                );
                            }
                            if resp.on_hover_text(tip).clicked() {
                                self.actions.push(Action::GoTo(d.clone()));
                            }
                        }
                    }
                });
                if let Some(op) = op {
                    self.place_op(op, false);
                }
            });
    }

    fn central(&mut self, ctx: &egui::Context) {
        let idle = self.idle();
        egui::CentralPanel::default().frame(egui::Frame::none().fill(CARD)).show(ctx, |ui| {
            let r = ui.max_rect();
            let mut outs = Vec::new();
            self.pane_rects = [None, None];
            self.zones = [None, None];
            if self.settings.dual {
                let mid = r.center().x.round();
                ui.painter().line_segment([pos2(mid, r.top()), pos2(mid, r.bottom())], Stroke::new(1.0, BORDER));
                let rects = [
                    Rect::from_min_max(r.min, pos2(mid - 0.5, r.max.y)),
                    Rect::from_min_max(pos2(mid + 0.5, r.min.y), r.max),
                ];
                for (i, pr) in rects.into_iter().enumerate() {
                    let active = self.active == i;
                    self.pane_rects[i] = Some(pr);
                    #[allow(deprecated)]
                    let out = ui.allocate_ui_at_rect(pr, |ui| pane_ui(ui, &mut self.sides[i], active, i, &mut self.settings.col_widths[i], self.settings.associated_icons.then_some(&mut self.file_icons))).inner;
                    outs.push((i, out));
                }
            } else {
                let i = self.active;
                self.pane_rects[i] = Some(r);
                #[allow(deprecated)]
                let out = ui.allocate_ui_at_rect(r, |ui| pane_ui(ui, &mut self.sides[i], true, i, &mut self.settings.col_widths[i], self.settings.associated_icons.then_some(&mut self.file_icons))).inner;
                outs.push((i, out));
            }
            for (i, out) in &mut outs {
                self.zones[*i] = Some(std::mem::take(&mut out.zone));
            }
            if outs.iter().any(|(_, o)| o.cols_changed) {
                self.save_settings();
            }
            if idle {
                for (i, out) in outs {
                    if let Some(sources) = out.drag_start {
                        self.active = i;
                        self.file_drag = Some(drag::FileDrag { sources, from: i });
                    }
                    if out.clicked || !out.actions.is_empty() {
                        self.active = i;
                    }
                    self.actions.extend(out.actions);
                }
            }
        });
    }

    // ───────────────────────────── overlays ─────────────────────────────

    fn modal_ui(&mut self, ctx: &egui::Context) {
        let mut modal = std::mem::replace(&mut self.modal, Modal::None);
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        let enter = ctx.input(|i| i.key_pressed(Key::Enter));
        let keep = match &mut modal {
            Modal::None => false,
            Modal::Help => {
                let mut close = esc || ctx.input(|i| i.key_pressed(Key::F1));
                dialog(ctx, "help", 560.0, |ui| {
                    dialog_header(ui, Glyph::Question, BLUE, "Горячие клавиши", Some("Quokka полностью управляется с клавиатуры"));
                    egui::Grid::new("help-grid").num_columns(4).spacing(vec2(10.0, 7.0)).show(ui, |ui| {
                        let rows: [(&str, &str, &str, &str); 19] = [
                            ("Ctrl+C", "Копировать в буфер", "Ctrl+X", "Вырезать"),
                            ("Ctrl+V", "Вставить в текущую папку", "", ""),
                            ("Alt+Enter", "Размеры папок", "", ""),
                            ("F1", "Справка", "Tab", "Другая панель"),
                            ("F2", "Переименовать", "Enter", "Открыть"),
                            ("F3", "Просмотр файла", "Backspace", "Папка выше"),
                            ("F4", "Во внешнем редакторе", "Insert", "Отметить и вниз"),
                            ("F5", "Копировать", "Space", "Отметить"),
                            ("F6", "Переместить", "Ctrl+A", "Отметить всё"),
                            ("F7", "Новая папка", "Esc", "Снять отметки"),
                            ("F8 / Del", "В корзину", "Ctrl+F", "Поиск"),
                            ("Shift+Del", "Удалить навсегда", "Alt+R", "Поиск по regex"),
                            ("F9", "Новый файл", "Ctrl+R", "Обновить"),
                            ("F10", "Выход", "Ctrl+T / W", "Новая / закрыть вкладку"),
                            ("F11", "Скрытые файлы", "Ctrl+L", "Ввести путь"),
                            ("F12", "Одна / две панели", "Alt+← →", "Назад / вперёд"),
                            ("Alt+F5", "Создать ZIP-архив", "Ctrl+D", "В избранное"),
                            ("Alt+F6", "Распаковать ZIP", "Ctrl+J", "Очередь операций"),
                            ("Ctrl+Z", "Отменить удаление", "Alt+F7", "Поиск файлов"),
                        ];
                        for (k1, d1, k2, d2) in rows {
                            kbd(ui, k1);
                            ui.label(RichText::new(d1).color(TEXT));
                            if k2.is_empty() {
                                ui.label("");
                            } else {
                                kbd(ui, k2);
                            }
                            ui.label(RichText::new(d2).color(TEXT));
                            ui.end_row();
                        }
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("Мышь: Ctrl+клик / Shift+клик — отметить, протянуть рамку с пустого места или со строки — выделить несколько, щелчок по пустому месту — снять выделение, двойной клик — открыть, правый клик — меню.").size(12.5).color(MUTED));
                    ui.add_space(10.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, "Закрыть", BLUE, Color32::WHITE).clicked() {
                            close = true;
                        }
                    });
                });
                !close
            }
            Modal::Settings { group } => {
                let mut close = esc;
                let before = self.settings.clone();
                let mut op = None;
                let mut reset = false;
                let looks: Vec<Look> = self.settings.places.clone().iter().map(|pl| place_look(&mut self.icon_cache, ctx, pl)).collect();
                dialog(ctx, "settings", 800.0_f32.min(ctx.screen_rect().width() - 76.0), |ui| {
                    dialog_header(ui, Glyph::Gear, BLUE, "Настройки", Some("Настройте Quokka под себя"));
                    let body_height = (ctx.screen_rect().height() - 228.0).clamp(240.0, 450.0);
                    ui.horizontal_top(|ui| {
                        settings_nav(ui, group);
                        ui.add_space(4.0);
                        let (sep, _) = ui.allocate_exact_size(vec2(1.0, body_height), Sense::hover());
                        ui.painter().rect_filled(sep, 0.0, BORDER);
                        ui.add_space(12.0);
                        ui.vertical(|ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .id_salt(("settings-content", *group as u8))
                                .max_height(body_height)
                                .min_scrolled_height(body_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.spacing_mut().item_spacing.y = 10.0;
                                    let st = &mut self.settings;
                                    match group {
                                        SettingsGroup::View => {
                                            section_title(ui, "Файлы", None);
                                            option(ui, &mut st.show_hidden, "Показывать скрытые файлы", "Файлы и папки с атрибутом «скрытый» или начинающиеся с точки. Клавиша F11.");
                                            if cfg!(windows) {
                                                option(ui, &mut st.associated_icons, "Иконки связанных приложений", "Использовать значки Windows для файлов в панелях и результатах поиска. Если значок недоступен, используется встроенный.");
                                            }
                                            ui.add_space(6.0);
                                            section_title(ui, "Окно", None);
                                            option(ui, &mut st.dual, "Две панели", "Показывать две файловые панели рядом. Клавиша F12.");
                                            ui.add_space(6.0);
                                            section_title(
                                                ui,
                                                "Внешний редактор (F4)",
                                                Some("Программа или команда; %f заменяется путём к файлу, иначе путь добавляется в конец. Пусто — по умолчанию. Встроенный редактор — Shift+F4."),
                                            );
                                            ui.add(
                                                egui::TextEdit::singleline(&mut st.editor)
                                                    .hint_text(fsops::default_editor_hint())
                                                    .desired_width(f32::INFINITY)
                                                    .margin(vec2(10.0, 8.0))
                                                    .font(mono(13.5)),
                                            );
                                        }
                                        SettingsGroup::Search => {
                                            section_title(ui, "Поиск по имени", Some("Фильтрует содержимое текущей папки (Ctrl+F)."));
                                            option(
                                                ui,
                                                &mut st.regex,
                                                "Регулярные выражения",
                                                "Например: ^IMG_\\d+\\.jpe?g$ или (отчёт|report).*\\.pdf. Переключатель: Alt+R или «.*» в строке поиска.",
                                            );
                                            option(ui, &mut st.case_sensitive, "Учитывать регистр букв", "«Отчёт» и «отчёт» будут считаться разными. Кнопка «Aa» в строке поиска.");
                                        }
                                        SettingsGroup::Sidebar => {
                                            section_title(ui, "Отображение", None);
                                            option(ui, &mut st.sidebar_labels, "Показывать подписи", "Широкое меню с названиями вместо одних значков.");
                                            option(ui, &mut st.show_drives, "Показывать диски", "Список дисков под закладками.");
                                            section_title(ui, "Избранное", Some("Порядок можно менять перетаскиванием прямо в боковом меню. Правый клик по значку — изменить или убрать."));
                                            let n = st.places.len();
                                            egui::Frame::none().fill(HOVER).rounding(12.0).inner_margin(Margin::same(8.0)).show(ui, |ui| {
                                                ui.spacing_mut().item_spacing.y = 4.0;
                                                for (i, pl) in st.places.iter().enumerate() {
                                                    ui.horizontal(|ui| {
                                                        ui.spacing_mut().item_spacing.x = 4.0;
                                                        let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                                                        draw_look(ui.painter(), r.shrink(3.0), looks[i], SLATE);
                                                        ui.vertical(|ui| {
                                                            ui.set_width((ui.available_width() - 124.0).max(80.0));
                                                            ui.spacing_mut().item_spacing.y = 0.0;
                                                            ui.add(egui::Label::new(RichText::new(&pl.name).font(semibold(13.0)).color(TEXT)).truncate());
                                                            let path = if pl.path.as_os_str().is_empty() { "Этот компьютер".to_string() } else { pl.path.display().to_string() };
                                                            ui.add(egui::Label::new(RichText::new(&path).size(11.5).color(TEXT_SOFT)).truncate()).on_hover_text(path);
                                                        });
                                                        if icon_btn(ui, Glyph::Pencil, SLATE, "Изменить").clicked() {
                                                            op = Some(PlaceOp::Edit(i));
                                                        }
                                                        if ui.add_enabled_ui(i > 0, |ui| icon_btn(ui, Glyph::ArrowUp, if i > 0 { SLATE } else { MUTED }, "Выше")).inner.clicked() {
                                                            op = Some(PlaceOp::Up(i));
                                                        }
                                                        if ui.add_enabled_ui(i + 1 < n, |ui| icon_btn(ui, Glyph::ChevronDown, if i + 1 < n { SLATE } else { MUTED }, "Ниже")).inner.clicked() {
                                                            op = Some(PlaceOp::Down(i));
                                                        }
                                                        if icon_btn(ui, Glyph::Trash, RED, "Убрать").clicked() {
                                                            op = Some(PlaceOp::Remove(i));
                                                        }
                                                    });
                                                }
                                            });
                                            ui.add_space(6.0);
                                            ui.horizontal_wrapped(|ui| {
                                                if small_btn(ui, "Добавить текущую папку", BLUE).clicked() {
                                                    op = Some(PlaceOp::Add);
                                                }
                                                if small_btn(ui, "Восстановить избранное", TEXT_SOFT).clicked() {
                                                    reset = true;
                                                }
                                            });
                                        }
                                    }
                            });
                        });
                    });
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Изменения сохраняются автоматически").size(12.0).color(TEXT_SOFT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if btn(ui, "Готово", BLUE, Color32::WHITE).clicked() {
                                close = true;
                            }
                        });
                    });
                });
                if reset {
                    self.settings.places = config::default_places();
                }
                if self.settings != before {
                    self.sync_settings();
                    self.save_settings();
                }
                if let Some(op) = op {
                    // edit/add open their own dialog and come back here afterwards
                    self.place_op(op, true);
                    if let Modal::PlaceEdit(_) = self.modal {
                        return;
                    }
                }
                !close
            }
            Modal::PlaceEdit(ed) => {
                let mut submit = false;
                let mut cancel = esc;
                let cache = &mut self.icon_cache;
                let title = if ed.index.is_some() { "Изменить избранное" } else { "Добавить в избранное" };
                dialog(ctx, "place-edit", 620.0, |ui| {
                    // header with a live preview of the chosen icon
                    let folder = PathBuf::from(ed.path.trim());
                    let preview = match ed.icon.as_deref() {
                        Some(spec) => Look { glyph: ed.glyph, tex: icon_texture(cache, ctx, spec, &folder, 64), tint: None },
                        None => Look { glyph: ed.glyph, tex: None, tint: ed.color.map(|[r, g, b]| Color32::from_rgb(r, g, b)) },
                    };
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                        ui.painter().rect_filled(r, 14.0, INPUT_BG);
                        draw_look(ui.painter(), r.shrink(12.0), preview, SLATE);
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new(title).font(semibold(18.0)).color(TEXT));
                            let what = match ed.icon.as_deref() {
                                Some(spec) => iconsrc::describe(spec),
                                None => "Стандартный значок".into(),
                            };
                            ui.label(RichText::new(what).size(13.0).color(MUTED));
                        });
                    });
                    ui.add_space(12.0);

                    ui.columns(2, |cols| {
                        cols[0].label(RichText::new("Название").color(TEXT_SOFT).size(13.0));
                        let r = cols[0].add(egui::TextEdit::singleline(&mut ed.name).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)).font(regular(15.0)));
                        if ed.first {
                            ed.first = false;
                            r.request_focus();
                        }
                        if r.lost_focus() && enter {
                            submit = true;
                        }
                        cols[1].label(RichText::new("Папка (пусто — «Этот компьютер»)").color(TEXT_SOFT).size(13.0));
                        let r = cols[1].add(egui::TextEdit::singleline(&mut ed.path).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)).font(regular(14.0)));
                        if r.lost_focus() && enter {
                            submit = true;
                        }
                    });
                    ui.add_space(10.0);

                    // source tabs
                    ui.horizontal(|ui| {
                        for (t, label) in [(IconTab::Builtin, "Стандартные"), (IconTab::Windows, "Значки Windows"), (IconTab::File, "Свой файл")] {
                            if pill(ui, label, ed.tab == t).clicked() && ed.tab != t {
                                ed.tab = t;
                                ed.page = 0;
                                ed.lib_count = None;
                                if t == IconTab::Windows && !ed.lib.starts_with(std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_default()) {
                                    ed.lib = iconsrc::system_lib("imageres.dll");
                                }
                            }
                        }
                    });
                    ui.add_space(8.0);
                    egui::Frame::none().fill(INPUT_BG).rounding(12.0).inner_margin(Margin::same(12.0)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(250.0);
                        match ed.tab {
                            IconTab::Builtin => {
                                let tint = ed.color.map(|[r, g, b]| Color32::from_rgb(r, g, b));
                                if icon_picker(ui, &mut ed.glyph, tint, ed.icon.is_none()) {
                                    ed.icon = None;
                                }
                                ui.add_space(10.0);
                                ui.label(RichText::new("Цвет значка").color(TEXT_SOFT).size(13.0));
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    for (c, name) in ICON_COLORS {
                                        let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                                        let on = ed.color == c;
                                        let p = ui.painter();
                                        match c {
                                            Some([cr, cg, cb]) => {
                                                p.circle_filled(r.center(), 11.0, Color32::from_rgb(cr, cg, cb));
                                            }
                                            None => {
                                                p.circle_stroke(r.center(), 10.5, Stroke::new(1.5, SLATE));
                                                p.line_segment([r.center() + vec2(-7.0, 7.0), r.center() + vec2(7.0, -7.0)], Stroke::new(1.5, SLATE));
                                            }
                                        }
                                        if on {
                                            p.circle_stroke(r.center(), 13.5, Stroke::new(2.0, BLUE));
                                        }
                                        if resp.on_hover_text(name).clicked() {
                                            ed.color = c;
                                            ed.icon = None;
                                        }
                                    }
                                    let mut rgb = ed.color.unwrap_or([22, 119, 255]);
                                    if ui.color_edit_button_srgb(&mut rgb).on_hover_text("Любой цвет").changed() {
                                        ed.color = Some(rgb);
                                        ed.icon = None;
                                    }
                                });
                            }
                            IconTab::Windows => {
                                ui.horizontal(|ui| {
                                    let cur = ed.lib.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                                    let label = iconsrc::WINDOWS_LIBS.iter().find(|(_, f)| *f == cur).map(|(l, _)| *l).unwrap_or("Библиотека");
                                    egui::ComboBox::from_id_salt("icon-lib").selected_text(label).width(260.0).show_ui(ui, |ui| {
                                        for (l, f) in iconsrc::WINDOWS_LIBS {
                                            if ui.selectable_label(cur == f, l).clicked() {
                                                ed.lib = iconsrc::system_lib(f);
                                                ed.lib_count = None;
                                                ed.page = 0;
                                            }
                                        }
                                    });
                                    let on = ed.icon.as_deref() == Some(iconsrc::SHELL);
                                    if pill(ui, "Значок самой папки", on).on_hover_text("Тот значок, что показывает Проводник (учитывает desktop.ini)").clicked() {
                                        ed.icon = Some(iconsrc::SHELL.into());
                                    }
                                });
                                ui.add_space(8.0);
                                resource_grid(ui, cache, ctx, ed);
                            }
                            IconTab::File => {
                                ui.horizontal(|ui| {
                                    if ui.button("Выбрать файл…").clicked() {
                                        if let Some(f) = iconsrc::pick_file() {
                                            if iconsrc::is_resource_file(&f) && iconsrc::count(&f) > 1 {
                                                ed.lib = f.clone();
                                                ed.lib_count = None;
                                                ed.page = 0;
                                                ed.icon = Some(iconsrc::make_spec(&f, 0));
                                            } else {
                                                ed.icon = Some(f.display().to_string());
                                            }
                                        }
                                    }
                                    ui.label(RichText::new("ico, png, jpg, svg, bmp, gif, webp, а также exe и dll").size(12.5).color(MUTED));
                                });
                                ui.add_space(8.0);
                                let own_lib = iconsrc::is_resource_file(&ed.lib)
                                    && !ed.lib.starts_with(std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_default());
                                if own_lib {
                                    ui.label(RichText::new(format!("Значки из {}", ed.lib.display())).size(12.5).color(TEXT_SOFT));
                                    resource_grid(ui, cache, ctx, ed);
                                } else if let Some(spec) = ed.icon.clone().filter(|s| s != iconsrc::SHELL) {
                                    ui.horizontal(|ui| {
                                        let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                                        match icon_texture(cache, ctx, &spec, &folder, 64) {
                                            Some(t) => {
                                                ui.painter().image(t, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                                            }
                                            None => {
                                                ui.painter().text(r.center(), Align2::CENTER_CENTER, "?", semibold(28.0), RED);
                                            }
                                        }
                                        ui.label(RichText::new(spec.as_str()).size(12.5).color(TEXT_SOFT));
                                    });
                                } else {
                                    ui.label(RichText::new("Файл не выбран").color(MUTED));
                                }
                            }
                        }
                    });
                    if let Some(e) = &ed.error {
                        ui.add_space(4.0);
                        ui.label(RichText::new(e.as_str()).color(RED).size(13.0));
                    }
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, "Сохранить", BLUE, Color32::WHITE).clicked() {
                            submit = true;
                        }
                        if btn(ui, "Отмена", INPUT_BG, TEXT).clicked() {
                            cancel = true;
                        }
                    });
                });
                let mut done = cancel;
                if submit {
                    let p = PathBuf::from(ed.path.trim());
                    if ed.name.trim().is_empty() {
                        ed.error = Some("Введите название".into());
                    } else if !p.as_os_str().is_empty() && !p.is_dir() {
                        ed.error = Some("Такой папки нет".into());
                    } else {
                        let place = Place {
                            name: ed.name.trim().to_string(),
                            path: p,
                            glyph: ed.glyph,
                            icon: ed.icon.clone(),
                            color: ed.color,
                        };
                        match ed.index {
                            Some(i) if i < self.settings.places.len() => self.settings.places[i] = place,
                            _ => self.settings.places.push(place),
                        }
                        self.save_settings();
                        done = true;
                    }
                }
                if done && ed.from_settings {
                    self.modal = Modal::Settings { group: SettingsGroup::Sidebar };
                }
                !done
            }
            Modal::Input { kind, title, label, text, first, error } => {
                let mut submit = false;
                let mut cancel = esc;
                let (glyph, color) = match kind {
                    InputKind::Zip(_) => (Glyph::Archive, ORANGE),
                    InputKind::Rename(_) => (Glyph::Pencil, BLUE),
                    InputKind::MkDir => (Glyph::FolderPlus, GREEN),
                    InputKind::NewFile => (Glyph::Plus, BLUE),
                };
                dialog(ctx, "input", 440.0, |ui| {
                    dialog_header(ui, glyph, color, title, Some(label));
                    let mut out = egui::TextEdit::singleline(text)
                        .desired_width(f32::INFINITY)
                        .margin(vec2(10.0, 8.0))
                        .font(regular(15.0))
                        .show(ui);
                    if *first {
                        *first = false;
                        out.response.request_focus();
                        let n = text.chars().count();
                        // a ZIP path may start with the destination folder: select just the file name
                        let start = match kind {
                            InputKind::Zip(_) => text.rfind(['\\', '/']).map_or(0, |i| text[..=i].chars().count()),
                            _ => 0,
                        };
                        let end = match (matches!(kind, InputKind::Rename(_) | InputKind::Zip(_)), text.rfind('.')) {
                            (true, Some(dot)) if dot > 0 && text[..dot].chars().count() > start => text[..dot].chars().count(),
                            _ => n,
                        };
                        out.state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(start),
                            egui::text::CCursor::new(end),
                        )));
                        out.state.store(ui.ctx(), out.response.id);
                    }
                    if out.response.lost_focus() && enter {
                        submit = true;
                    }
                    if let Some(e) = error {
                        ui.add_space(4.0);
                        ui.label(RichText::new(e.as_str()).color(RED).size(13.0));
                    }
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, "ОК", BLUE, Color32::WHITE).clicked() {
                            submit = true;
                        }
                        if btn(ui, "Отмена", INPUT_BG, TEXT).clicked() {
                            cancel = true;
                        }
                    });
                });
                if submit {
                    match self.apply_input(kind, text) {
                        Ok(()) => false,
                        Err(e) => {
                            *error = Some(e);
                            true
                        }
                    }
                } else {
                    !cancel
                }
            }
            Modal::Transfer { mv, sources, dest, overwrite, first } => {
                let mut submit = false;
                let mut cancel = esc;
                let what = if sources.len() == 1 {
                    format!("\u{201c}{}\u{201d}", sources[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                } else {
                    items_text(sources.len() as u64)
                };
                let (title, glyph, color) = if *mv { ("Переместить", Glyph::ArrowRight, GREEN) } else { ("Копировать", Glyph::Copy, BLUE) };
                dialog(ctx, "transfer", 520.0, |ui| {
                    dialog_header(ui, glyph, color, &format!("{title} {what}"), Some("Папка назначения"));
                    let r = ui.add(egui::TextEdit::singleline(dest).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)).font(regular(15.0)));
                    if *first {
                        *first = false;
                        r.request_focus();
                    }
                    if r.lost_focus() && enter {
                        submit = true;
                    }
                    ui.add_space(8.0);
                    ui.checkbox(overwrite, "Заменять существующие файлы");
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, title, color, Color32::WHITE).clicked() {
                            submit = true;
                        }
                        if btn(ui, "Отмена", INPUT_BG, TEXT).clicked() {
                            cancel = true;
                        }
                    });
                });
                if submit {
                    let mut d = PathBuf::from(dest.trim());
                    if d.as_os_str().is_empty() {
                        self.toast("Укажите папку назначения", true);
                        true
                    } else {
                        if d.is_relative() {
                            d = self.ap_ref().path.join(d);
                        }
                        let sources = std::mem::take(sources);
                        let overwrite = *overwrite;
                        self.enqueue(if *mv {
                            OpSpec::Move { sources, dest: d, overwrite }
                        } else {
                            OpSpec::Copy { sources, dest: d, overwrite }
                        });
                        false
                    }
                } else {
                    !cancel
                }
            }
            Modal::ConfirmDelete { sources, permanent } => {
                let mut submit = enter;
                let mut cancel = esc;
                let permanent = *permanent;
                dialog(ctx, "delete", 440.0, |ui| {
                    let n = items_text(sources.len() as u64);
                    let (title, sub) = if permanent {
                        ("Удалить навсегда", format!("{n} будет удалено без возможности восстановления"))
                    } else {
                        ("Удалить в корзину", format!("{n} будет перемещено в корзину Windows"))
                    };
                    dialog_header(ui, Glyph::Trash, RED, title, Some(&sub));
                    for s in sources.iter().take(6) {
                        let name = s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        ui.label(RichText::new(format!("•  {name}")).color(TEXT));
                    }
                    if sources.len() > 6 {
                        ui.label(RichText::new(format!("…и ещё {}", sources.len() - 6)).color(MUTED));
                    }
                    ui.add_space(6.0);
                    let note = if permanent {
                        "Это действие нельзя отменить."
                    } else {
                        "Файлы можно будет восстановить из корзины. Shift+Del — удалить навсегда."
                    };
                    ui.label(RichText::new(note).color(MUTED).size(13.0));
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, if permanent { "Удалить" } else { "В корзину" }, RED, Color32::WHITE).clicked() {
                            submit = true;
                        }
                        if btn(ui, "Отмена", INPUT_BG, TEXT).clicked() {
                            cancel = true;
                        }
                    });
                });
                if submit && !cancel {
                    let sources = std::mem::take(sources);
                    self.undo_recycle = None;
                    self.enqueue(if permanent { OpSpec::Delete { sources } } else { OpSpec::Recycle { sources } });
                    false
                } else {
                    !cancel
                }
            }
            Modal::Viewer { path, content, info } => {
                let mut close = esc || ctx.input(|i| i.key_pressed(Key::F3) || i.key_pressed(Key::F10));
                let screen = ctx.screen_rect();
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                dialog(ctx, "viewer", (screen.width() * 0.8).min(1200.0), |ui| {
                    dialog_header(ui, Glyph::Eye, BLUE, &name, Some(info));
                    egui::Frame::none().fill(INPUT_BG).rounding(10.0).inner_margin(Margin::same(10.0)).show(ui, |ui| {
                        egui::ScrollArea::both().max_height(screen.height() * 0.62).auto_shrink([false, false]).show(ui, |ui| {
                            let mut s: &str = content.as_str();
                            ui.add(egui::TextEdit::multiline(&mut s).font(mono(13.0)).desired_width(f32::INFINITY).frame(false));
                        });
                    });
                    ui.add_space(12.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, "Закрыть", BLUE, Color32::WHITE).clicked() {
                            close = true;
                        }
                        ui.label(RichText::new("F3 / Esc — закрыть").color(MUTED).size(12.5));
                    });
                });
                !close
            }
            Modal::Editor { path, text, original, confirm_close } => {
                let dirty = text != original;
                let mut save = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S) || i.consume_key(Modifiers::NONE, Key::F2));
                let mut close = esc || ctx.input(|i| i.key_pressed(Key::F10));
                let mut discard = false;
                let screen = ctx.screen_rect();
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let sub = if dirty { "Изменён — Ctrl+S, чтобы сохранить".to_string() } else { path.display().to_string() };
                dialog(ctx, "editor", (screen.width() * 0.8).min(1200.0), |ui| {
                    dialog_header(ui, Glyph::Pencil, if dirty { ORANGE } else { BLUE }, &name, Some(&sub));
                    egui::Frame::none().fill(INPUT_BG).rounding(10.0).inner_margin(Margin::same(10.0)).show(ui, |ui| {
                        egui::ScrollArea::both().max_height(screen.height() * 0.6).auto_shrink([false, false]).show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(text)
                                    .code_editor()
                                    .font(mono(13.0))
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(28)
                                    .frame(false),
                            );
                        });
                    });
                    ui.add_space(12.0);
                    buttons_row(ui, |ui| {
                        if *confirm_close {
                            if btn(ui, "Сохранить и закрыть", BLUE, Color32::WHITE).clicked() {
                                save = true;
                                discard = true;
                            }
                            if btn(ui, "Не сохранять", RED, Color32::WHITE).clicked() {
                                discard = true;
                                *original = text.clone();
                            }
                            if btn(ui, "Отмена", INPUT_BG, TEXT).clicked() {
                                *confirm_close = false;
                            }
                            ui.label(RichText::new("Есть несохранённые изменения").color(ORANGE));
                        } else {
                            if btn(ui, "Закрыть", INPUT_BG, TEXT).clicked() {
                                close = true;
                            }
                            if btn(ui, "Сохранить", BLUE, Color32::WHITE).clicked() {
                                save = true;
                            }
                        }
                    });
                });
                let mut keep = true;
                if save {
                    match fs::write(&*path, text.as_bytes()) {
                        Ok(()) => {
                            *original = text.clone();
                            self.toast(format!("Сохранено: {name}"), false);
                            self.reload_all();
                        }
                        Err(e) => {
                            self.toast(format!("Не удалось сохранить: {e}"), true);
                            discard = false;
                        }
                    }
                }
                if discard {
                    keep = false;
                } else if close {
                    if text != original {
                        *confirm_close = true;
                    } else {
                        keep = false;
                    }
                }
                keep
            }
            Modal::Unzip { archives, dest, subfolder, overwrite, first } => {
                let mut submit = false;
                let mut cancel = esc;
                let single = (archives.len() == 1).then(|| file_stem_of(&archives[0]));
                let title = match &single {
                    Some(_) => format!("Распаковать «{}»", archives[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                    None => format!("Распаковать {}", items_text(archives.len() as u64)),
                };
                dialog(ctx, "unzip", 520.0, |ui| {
                    dialog_header(ui, Glyph::Archive, ORANGE, &title, Some("Папка назначения"));
                    let r = ui.add(egui::TextEdit::singleline(dest).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)).font(regular(15.0)));
                    if *first {
                        *first = false;
                        r.request_focus();
                    }
                    if r.lost_focus() && enter {
                        submit = true;
                    }
                    ui.add_space(8.0);
                    let sub = match &single {
                        Some(stem) => format!("Создать папку «{stem}»"),
                        None => "Каждый архив — в отдельную папку".to_string(),
                    };
                    ui.checkbox(subfolder, sub);
                    ui.checkbox(overwrite, "Заменять существующие файлы");
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, "Распаковать", ORANGE, Color32::WHITE).clicked() {
                            submit = true;
                        }
                        if btn(ui, "Отмена", INPUT_BG, TEXT).clicked() {
                            cancel = true;
                        }
                    });
                });
                if submit {
                    let mut d = PathBuf::from(dest.trim());
                    if d.as_os_str().is_empty() {
                        self.toast("Укажите папку назначения", true);
                        true
                    } else {
                        if d.is_relative() {
                            d = self.ap_ref().path.join(d);
                        }
                        let (subfolder, overwrite) = (*subfolder, *overwrite);
                        for archive in std::mem::take(archives) {
                            let dest = if subfolder { d.join(file_stem_of(&archive)) } else { d.clone() };
                            self.enqueue(OpSpec::Unzip { archive, dest, overwrite });
                        }
                        false
                    }
                } else {
                    !cancel
                }
            }
            Modal::ConfirmQuit => {
                let mut stay = esc;
                let mut quit = false;
                let n = self.queue.pending_count() + usize::from(self.queue.running().is_some());
                dialog(ctx, "quit", 440.0, |ui| {
                    dialog_header(ui, Glyph::Question, ORANGE, "Операции ещё выполняются", Some(&format!("Незавершённых операций: {n}")));
                    ui.label(RichText::new("Если выйти сейчас, текущая операция будет прервана, а ожидающие — отменены.").color(TEXT));
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, "Прервать и выйти", RED, Color32::WHITE).clicked() {
                            quit = true;
                        }
                        if btn(ui, "Остаться", INPUT_BG, TEXT).clicked() {
                            stay = true;
                        }
                    });
                });
                if quit {
                    self.queue.cancel_all();
                    self.quitting = true;
                    false
                } else {
                    !stay
                }
            }
            Modal::Find(fd) => self.find_ui(ctx, fd, esc, enter),
            Modal::Message { title, text } => {
                let mut close = esc || enter;
                dialog(ctx, "message", 420.0, |ui| {
                    dialog_header(ui, Glyph::Question, ORANGE, title, None);
                    ui.label(RichText::new(text.as_str()).color(TEXT));
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, "ОК", BLUE, Color32::WHITE).clicked() {
                            close = true;
                        }
                    });
                });
                !close
            }
        };
        if keep && matches!(self.modal, Modal::None) {
            self.modal = modal;
        } else if let Modal::Find(fd) = modal {
            self.find = Some(fd);
        }
        if self.resume_find && matches!(self.modal, Modal::None) {
            self.resume_find = false;
            if let Some(fd) = self.find.take() {
                self.modal = Modal::Find(fd);
            }
        }
    }

    /// Floating queue card above the F-key bar: compact while collapsed, full list when open.
    fn queue_ui(&mut self, ctx: &egui::Context) {
        let active = self.queue.is_active();
        if !self.queue_open && !active {
            return;
        }
        if active {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut ops = Vec::new();
        let open = self.queue_open;
        let queue = &self.queue;
        egui::Area::new(Id::new("queue"))
            .order(egui::Order::Middle)
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -66.0))
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(CARD)
                    .rounding(14.0)
                    .stroke(Stroke::new(1.0, BORDER))
                    .shadow(ctx.style().visuals.popup_shadow)
                    .inner_margin(Margin::same(14.0))
                    .show(ui, |ui| {
                        ui.set_width(420.0);
                        if open {
                            queue_full(ui, queue, &mut ops);
                        } else {
                            queue_compact(ui, queue, &mut ops);
                        }
                    });
            });
        self.apply_queue_ops(ops);
    }

    fn apply_queue_ops(&mut self, ops: Vec<QueueOp>) {
        for op in ops {
            match op {
                QueueOp::Open => self.queue_open = true,
                QueueOp::Close => self.queue_open = false,
                QueueOp::Pause => self.queue.paused = !self.queue.paused,
                QueueOp::ClearFinished => self.queue.clear_finished(),
                QueueOp::Cancel(id) => self.queue.cancel(id),
                QueueOp::Retry(id) => self.queue.retry(id),
                QueueOp::Remove(id) => self.queue.remove(id),
                QueueOp::Up(id) => self.queue.shift(id, true),
                QueueOp::Down(id) => self.queue.shift(id, false),
            }
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let mut undo = false;
        let Some(t) = &mut self.toast else { return };
        let born = *t.born.get_or_insert(now);
        let life = if t.undo { 8.0 } else if t.error { 4.5 } else { 2.5 };
        if now - born > life {
            self.toast = None;
            return;
        }
        let age = (now - born) as f32;
        let fade_in = ease_out_cubic((age / 0.25).min(1.0));
        let fade_out = ((life as f32 - age) / 0.35).clamp(0.0, 1.0);
        let opacity = fade_in * ease_out_cubic(fade_out);
        let rise = 18.0 * (1.0 - ease_out_back((age / 0.35).min(1.0)));
        egui::Area::new(Id::new("toast"))
            .order(egui::Order::Tooltip)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -70.0 + rise))
            .interactable(t.undo)
            .fade_in(false)
            .show(ctx, |ui| {
                ui.multiply_opacity(opacity);
                egui::Frame::none()
                    .fill(if t.error { RED } else { TEXT })
                    .rounding(10.0)
                    .inner_margin(Margin::symmetric(16.0, 10.0))
                    .shadow(ctx.style().visuals.popup_shadow)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(t.text.as_str()).color(Color32::WHITE).size(14.0));
                            if t.undo {
                                ui.add_space(10.0);
                                let b = egui::Button::new(RichText::new("Отменить").font(semibold(14.0)).color(BLUE_SOFT))
                                    .fill(Color32::from_white_alpha(28))
                                    .stroke(Stroke::NONE)
                                    .rounding(7.0);
                                if ui.add(b).on_hover_text("Вернуть из корзины (Ctrl+Z)").clicked() {
                                    undo = true;
                                }
                            }
                        });
                    });
            });
        if undo {
            self.actions.push(Action::UndoDelete);
            return;
        }
        if age < 0.4 || life as f32 - age < 0.4 {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f32(life as f32 - age - 0.35));
        }
    }
}

impl eframe::App for QuokkaApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // egui-winit swallows Ctrl+V when the native clipboard has files but no
        // text. Recover it from the native key state. Key-up also catches taps
        // that were released before this frame, including on a Cyrillic layout.
        let down = raw.focused && crate::clipboard::paste_down();
        let released = raw.focused && raw.events.iter().any(|e| matches!(e,
            egui::Event::Key { key: Key::V, pressed: false, modifiers, .. } if modifiers.command));
        if (down || released) && !self.paste_held && self.idle() && !self.text_focus(ctx)
            && !raw.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
        {
            raw.events.push(egui::Event::Paste(String::new()));
        }
        self.paste_held = down;
        if down { ctx.request_repaint_after(std::time::Duration::from_millis(16)); }
        if std::mem::take(&mut self.release_pointer) {
            raw.events.push(egui::Event::PointerButton {
                pos: pos2(-1.0, -1.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
            raw.events.push(egui::Event::PointerGone);
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        begin_row_clicks(ctx);
        if self.hwnd == 0 {
            self.hwnd = crate::dnd::window_handle(frame);
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.queue.is_active() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if matches!(self.modal, Modal::None) {
                self.modal = Modal::ConfirmQuit;
            } else {
                self.toast("Идут операции — закройте диалог, чтобы выйти", true);
            }
        }
        self.poll_queue(ctx);
        let opening = ctx.data(|d| d.get_temp::<OpenFeedback>(open_feedback_id()));
        if let Some(opening) = opening {
            if ctx.input(|i| i.time) - opening.started >= OPEN_FEEDBACK_SECS {
                ctx.data_mut(|d| d.remove::<OpenFeedback>(open_feedback_id()));
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        for side in &mut self.sides {
            for panel in &mut side.tabs { panel.poll_sizes(); }
        }
        if self.quitting && !self.queue.is_active() {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.idle() {
            self.handle_keys(ctx);
        }
        self.top_bar(ctx);
        self.fkey_bar(ctx);
        self.sidebar(ctx);
        self.central(ctx);
        self.drag_and_drop(ctx);
        self.modal_ui(ctx);
        self.queue_ui(ctx);
        self.toast_ui(ctx);
        backdrop(ctx, !matches!(self.modal, Modal::None));
        for a in std::mem::take(&mut self.actions) {
            self.run(a, ctx);
        }
        self.focus_policy(ctx);
    }
}

// ───────────────────────────── pane ─────────────────────────────

struct Cols {
    name: (f32, f32),
    modified: Option<(f32, f32)>,
    size: (f32, f32),
    kind: Option<(f32, f32)>,
}

/// The «Имя» column never gets narrower than this; optional columns are hidden first.
const MIN_NAME_W: f32 = 180.0;

impl Cols {
    /// `widths` = «Изменён», «Размер», «Тип» (see `Settings::col_widths`).
    fn new(r: Rect, widths: &[f32; 3]) -> Self {
        Self::with_visibility(r, widths, None)
    }

    fn with_visibility(r: Rect, widths: &[f32; 3], visibility: Option<(bool, bool)>) -> Self {
        let w = r.width();
        let [wm, ws, wk] = *widths;
        let show_kind = w - (wm + ws + wk) >= MIN_NAME_W;
        let show_mod = w - (wm + ws + if show_kind { wk } else { 0.0 }) >= MIN_NAME_W;
        let (show_mod, show_kind) = visibility.unwrap_or((show_mod, show_kind));
        let mut right = r.right();
        let kind = if show_kind {
            let c = (right - wk, right);
            right -= wk;
            Some(c)
        } else {
            None
        };
        let ws = ws.min((right - r.left() - 80.0).max(config::MIN_COL_WIDTH));
        let size = (right - ws, right);
        right -= ws;
        let modified = if show_mod {
            let c = (right - wm, right);
            right -= wm;
            Some(c)
        } else {
            None
        };
        Cols { name: (r.left(), right), modified, size, kind }
    }
}

fn pane_ui(ui: &mut Ui, side: &mut Side, active: bool, idx: usize, col_widths: &mut [f32; 3], file_icons: Option<&mut crate::file_icons::FileIcons>) -> PaneOut {
    let mut out = PaneOut::default();
    let full = ui.max_rect();
    if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(full) {
        out.clicked = true;
    }
    let tabs_r = Rect::from_min_size(full.min, vec2(full.width(), TABS_H));
    let tool_r = Rect::from_min_size(pos2(full.left(), tabs_r.bottom()), vec2(full.width(), TOOL_H));
    let foot_r = Rect::from_min_max(pos2(full.left(), full.bottom() - FOOT_H), full.max);
    let table_r = Rect::from_min_max(pos2(full.left() + 8.0, tool_r.bottom()), pos2(full.right() - 8.0, foot_r.top()));

    tab_strip(ui, side, active, idx, tabs_r, &mut out);
    let panel = side.panel_mut();
    toolbar(ui, panel, idx, tool_r, &mut out);
    table(ui, panel, active, idx, table_r, col_widths, &mut out, file_icons);
    out.zone.area = full;
    footer(ui, panel, foot_r);
    out
}

fn tab_strip(ui: &mut Ui, side: &mut Side, active: bool, idx: usize, rect: Rect, out: &mut PaneOut) {
    const GAP: f32 = 6.0;
    const MIN_W: f32 = 120.0;
    const MAX_W: f32 = 230.0;
    let p = ui.painter().clone();
    p.line_segment([pos2(rect.left(), rect.bottom() - 0.5), pos2(rect.right(), rect.bottom() - 0.5)], Stroke::new(1.0, BORDER));
    let n = side.tabs.len();
    let cy = rect.top() + 8.0 + (rect.height() - 8.0) / 2.0;

    // Tabs shrink to fit; below MIN_W the strip scrolls instead of spilling
    // into the neighbouring pane. The "+" (and the tab list) stay pinned on the right.
    let full = rect.width() - 10.0 - 44.0;
    let fit_w = full / n as f32 - GAP;
    let overflow = fit_w < MIN_W;
    let tab_w = fit_w.clamp(MIN_W, MAX_W);
    let buttons_w = if overflow { 76.0 } else { 44.0 };
    let strip = Rect::from_min_max(pos2(rect.left() + 10.0, rect.top()), pos2(rect.right() - buttons_w, rect.bottom()));
    let content_w = n as f32 * (tab_w + GAP);
    let max_scroll = (content_w - strip.width()).max(0.0);

    // mouse wheel over the strip scrolls it (vertical wheel works too)
    let strip_resp = ui.interact(strip, Id::new(("tab-strip", idx)), Sense::hover());
    if overflow && strip_resp.hovered() {
        let d = ui.input(|i| i.smooth_scroll_delta);
        let delta = if d.x.abs() > d.y.abs() { d.x } else { d.y };
        side.tab_scroll -= delta;
    }
    if side.cur != side.last_cur {
        let x0 = side.cur as f32 * (tab_w + GAP);
        if x0 < side.tab_scroll {
            side.tab_scroll = x0;
        } else if x0 + tab_w > side.tab_scroll + strip.width() {
            side.tab_scroll = x0 + tab_w - strip.width();
        }
        side.last_cur = side.cur;
    }
    side.tab_scroll = side.tab_scroll.clamp(0.0, max_scroll);

    let pc = p.with_clip_rect(strip);
    let (mut select, mut close) = (None, None);
    let mut x = strip.left() - side.tab_scroll;
    for i in 0..n {
        let tr = Rect::from_min_max(pos2(x, rect.top() + 8.0), pos2(x + tab_w, rect.bottom()));
        x += tab_w + GAP;
        let vis = tr.intersect(strip);
        if vis.width() <= 0.0 {
            continue;
        }
        let resp = ui.interact(vis, Id::new(("tab", idx, i)), Sense::click());
        let is_cur = i == side.cur;
        let fill = if is_cur {
            if active {
                TAB_ACTIVE
            } else {
                SEL_INACTIVE
            }
        } else if resp.hovered() {
            HOVER
        } else {
            Color32::TRANSPARENT
        };
        pc.rect_filled(tr, Rounding { nw: 10.0, ne: 10.0, sw: 0.0, se: 0.0 }, fill);
        icons::folder(&pc, Rect::from_center_size(pos2(tr.left() + 22.0, tr.center().y), vec2(20.0, 18.0)));
        let text_clip = Rect::from_min_max(tr.min, pos2(tr.right() - if n > 1 { 32.0 } else { 8.0 }, tr.bottom())).intersect(strip);
        p.with_clip_rect(text_clip).text(
            pos2(tr.left() + 40.0, tr.center().y),
            Align2::LEFT_CENTER,
            side.tabs[i].title(),
            if is_cur { semibold(14.5) } else { regular(14.5) },
            if is_cur { TEXT } else { TEXT_SOFT },
        );
        let resp = resp.on_hover_text(side.tabs[i].path.display().to_string());
        if n > 1 {
            let cr = Rect::from_center_size(pos2(tr.right() - 18.0, tr.center().y), vec2(22.0, 22.0));
            if strip.contains_rect(cr) {
                let cresp = ui.interact(cr, Id::new(("tab-x", idx, i)), Sense::click());
                if cresp.hovered() {
                    pc.rect_filled(cr, 6.0, Color32::from_black_alpha(14));
                }
                icons::glyph(&pc, cr.shrink(4.0), Glyph::Close, TEXT_SOFT);
                if cresp.clicked() {
                    close = Some(i);
                }
            }
        }
        if resp.clicked() {
            select = Some(i);
        }
        if resp.middle_clicked() {
            close = Some(i);
        }
    }

    // soft fade at clipped edges so it is clear there are more tabs
    if overflow {
        let fade = |x0: f32, x1: f32, left: bool| {
            let steps = 12;
            for k in 0..steps {
                let t = k as f32 / steps as f32;
                let a = if left { 1.0 - t } else { t };
                let r = Rect::from_min_max(
                    pos2(x0 + (x1 - x0) * t, rect.top() + 8.0),
                    pos2(x0 + (x1 - x0) * (t + 1.0 / steps as f32), rect.bottom() - 1.0),
                );
                p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, (a * 230.0) as u8));
            }
        };
        if side.tab_scroll > 0.5 {
            fade(strip.left(), strip.left() + 24.0, true);
        }
        if side.tab_scroll < max_scroll - 0.5 {
            fade(strip.right() - 24.0, strip.right(), false);
        }
    }

    let plus_x = if overflow { rect.right() - 58.0 } else { (x + 18.0).min(rect.right() - 22.0) };
    let pr = Rect::from_center_size(pos2(plus_x, cy), vec2(30.0, 30.0));
    let presp = ui.interact(pr, Id::new(("tab-plus", idx)), Sense::click());
    if presp.hovered() {
        p.rect_filled(pr, 8.0, HOVER);
    }
    icons::glyph(&p, pr.shrink(6.0), Glyph::Plus, TEXT_SOFT);
    if presp.on_hover_text("Новая вкладка (Ctrl+T)").clicked() {
        out.actions.push(Action::NewTab);
    }

    // with many tabs: a drop-down listing all of them
    if overflow {
        let lr = Rect::from_center_size(pos2(rect.right() - 22.0, cy), vec2(30.0, 30.0));
        let lresp = ui.interact(lr, Id::new(("tab-list", idx)), Sense::click());
        if lresp.hovered() {
            p.rect_filled(lr, 8.0, HOVER);
        }
        icons::glyph(&p, lr.shrink(7.0), Glyph::ChevronDown, TEXT_SOFT);
        let popup = Id::new(("tab-list-popup", idx));
        if lresp.clone().on_hover_text(format!("Все вкладки ({n})")).clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup));
        }
        egui::popup::popup_below_widget(ui, popup, &lresp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(260.0);
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for i in 0..n {
                    if ui.selectable_label(i == side.cur, side.tabs[i].title()).clicked() {
                        select = Some(i);
                    }
                }
            });
        });
    }

    if let Some(i) = select {
        side.cur = i;
    }
    if let Some(i) = close {
        if side.tabs.len() > 1 {
            side.tabs.remove(i);
            if side.cur > i || side.cur >= side.tabs.len() {
                side.cur = side.cur.saturating_sub(1);
            }
        }
    }
}

fn tool_btn(ui: &Ui, rect: Rect, id: Id, g: Glyph, enabled: bool) -> Response {
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    p.rect(rect, 8.0, if resp.hovered() && enabled { HOVER } else { CARD }, Stroke::new(1.0, BORDER));
    icons::glyph(p, rect.shrink(8.0), g, if enabled { TEXT_SOFT } else { Color32::from_rgb(200, 206, 216) });
    resp
}

fn toolbar(ui: &mut Ui, panel: &mut Panel, idx: usize, rect: Rect, out: &mut PaneOut) {
    let cy = rect.center().y;
    let b = 34.0;
    let mut x = rect.left() + 10.0;
    let specs = [
        (Glyph::ArrowLeft, !panel.back.is_empty(), Action::Back),
        (Glyph::ArrowRight, !panel.forward.is_empty(), Action::Forward),
        (Glyph::ArrowUp, parent_of(&panel.path).is_some(), Action::GoUp),
    ];
    for (k, (g, en, a)) in specs.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, cy - b / 2.0), vec2(b, b));
        if tool_btn(ui, r, Id::new(("tool", idx, k)), g, en).clicked() {
            out.actions.push(a);
        }
        x += b + 8.0;
    }
    let p = ui.painter().clone();
    p.line_segment([pos2(x + 2.0, cy - 12.0), pos2(x + 2.0, cy + 12.0)], Stroke::new(1.0, BORDER));
    x += 12.0;

    let rr = Rect::from_min_size(pos2(rect.right() - 10.0 - b, cy - b / 2.0), vec2(b, b));
    if tool_btn(ui, rr, Id::new(("tool", idx, 9)), Glyph::Refresh, true).on_hover_text("Обновить (Ctrl+R)").clicked() {
        out.actions.push(Action::Refresh);
    }

    let crumb_r = Rect::from_min_max(pos2(x, cy - 18.0), pos2(rr.left() - 8.0, cy + 18.0));
    let editing = panel.editing_path.is_some();
    p.rect(crumb_r, 8.0, CARD, Stroke::new(if editing { 1.5 } else { 1.0 }, if editing { BLUE } else { BORDER }));

    if let Some(text) = panel.editing_path.as_mut() {
        let tr = crumb_r.shrink2(vec2(12.0, 6.0));
        let r = ui.put(
            tr,
            egui::TextEdit::singleline(text).id(path_edit_id(idx)).frame(false).font(regular(14.5)).text_color(TEXT),
        );
        if panel.focus_path_edit {
            r.request_focus();
            panel.focus_path_edit = false;
        }
        if r.lost_focus() {
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                let t = panel.editing_path.take().unwrap_or_default();
                out.actions.push(Action::GoTo(PathBuf::from(t.trim())));
            } else {
                panel.editing_path = None;
            }
        }
        return;
    }

    // whole box: double-click to type a path
    let box_resp = ui.interact(crumb_r, Id::new(("crumb-box", idx)), Sense::click());
    if box_resp.double_clicked() {
        out.actions.push(Action::EditPath);
    }

    // segments
    let mut segs: Vec<(String, PathBuf)> = Vec::new();
    if cfg!(windows) {
        segs.push(("Этот компьютер".into(), PathBuf::new()));
    }
    if !panel.is_pc() {
        let mut anc: Vec<PathBuf> = panel.path.ancestors().map(|a| a.to_path_buf()).collect();
        anc.reverse();
        for a in anc {
            let label = a.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root_label(&a));
            segs.push((label, a));
        }
    }
    let font = regular(14.5);
    let galleys: Vec<_> = segs.iter().map(|(l, _)| p.layout_no_wrap(l.clone(), font.clone(), TEXT)).collect();
    let icon_w = 30.0;
    let chev_w = 20.0;
    let avail = crumb_r.width() - icon_w - 44.0;
    let mut start = 0;
    let total = |s: usize| -> f32 { galleys[s..].iter().map(|g| g.size().x + chev_w).sum::<f32>() + if s > 0 { 24.0 } else { 0.0 } };
    while start + 1 < segs.len() && total(start) > avail {
        start += 1;
    }
    icons::glyph(&p, Rect::from_center_size(pos2(crumb_r.left() + 20.0, cy), vec2(20.0, 20.0)), Glyph::Desktop, BLUE);
    let clip = crumb_r.shrink2(vec2(4.0, 0.0));
    let pc = p.with_clip_rect(Rect::from_min_max(clip.min, pos2(crumb_r.right() - 34.0, clip.max.y)));
    let mut sx = crumb_r.left() + icon_w + 8.0;
    if start > 0 {
        pc.text(pos2(sx, cy), Align2::LEFT_CENTER, "…", font.clone(), MUTED);
        sx += 24.0;
    }
    let last = segs.len() - 1;
    for (i, ((_, path), g)) in segs.iter().zip(galleys).enumerate().skip(start) {
        let gr = Rect::from_min_size(pos2(sx - 4.0, cy - 13.0), vec2(g.size().x + 8.0, 26.0));
        let resp = ui.interact(gr, Id::new(("crumb", idx, i)), Sense::click());
        if resp.hovered() {
            pc.rect_filled(gr, 6.0, HOVER);
        }
        let color = if i == last { TEXT } else if resp.hovered() { BLUE } else { TEXT_SOFT };
        pc.galley(pos2(sx, cy - g.size().y / 2.0), g.clone(), color);
        if resp.clicked() && i != last {
            out.actions.push(Action::GoTo(path.clone()));
        }
        sx += g.size().x;
        if i != last {
            icons::glyph(&pc, Rect::from_center_size(pos2(sx + chev_w / 2.0, cy), vec2(12.0, 12.0)), Glyph::ChevronRight, MUTED);
            sx += chev_w;
        }
    }
    let dr = Rect::from_center_size(pos2(crumb_r.right() - 18.0, cy), vec2(24.0, 24.0));
    let dresp = ui.interact(dr, Id::new(("crumb-edit", idx)), Sense::click());
    if dresp.hovered() {
        p.rect_filled(dr, 6.0, HOVER);
    }
    icons::glyph(&p, dr.shrink(4.0), Glyph::ChevronDown, TEXT_SOFT);
    if dresp.on_hover_text("Ввести путь (Ctrl+L)").clicked() {
        out.actions.push(Action::EditPath);
    }
}

/// Common look for right-click menus: roomy spacing and soft hover rows.
fn menu_style(ui: &mut Ui, min_width: f32) {
    ui.set_min_width(min_width);
    let s = ui.style_mut();
    s.spacing.item_spacing = vec2(0.0, 2.0);
    s.spacing.button_padding = vec2(BUTTON_PADDING_X, BUTTON_PADDING_Y);
    s.spacing.interact_size.y = 34.0;
    let w = &mut s.visuals.widgets;
    for wv in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.rounding = Rounding::same(8.0);
        wv.bg_stroke = Stroke::NONE;
        wv.expansion = 0.0;
    }
    w.hovered.weak_bg_fill = BLUE_SOFT;
    w.hovered.bg_fill = BLUE_SOFT;
    w.active.weak_bg_fill = Color32::from_rgb(218, 232, 255);
    w.active.bg_fill = Color32::from_rgb(218, 232, 255);
    w.open.weak_bg_fill = BLUE_SOFT;
    w.open.bg_fill = BLUE_SOFT;
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_fill = Color32::TRANSPARENT;
}

/// Thin separator with extra breathing room between menu groups.
fn menu_sep(ui: &mut Ui) {
    ui.add_space(3.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment([pos2(r.left() + 10.0, r.center().y), pos2(r.right() - 10.0, r.center().y)], Stroke::new(1.0, BORDER));
    ui.add_space(3.0);
}

/// Small muted caption at the top of a menu.
fn menu_title(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(BUTTON_PADDING_X);
        ui.label(RichText::new(text).color(MUTED).size(12.0));
    });
    ui.add_space(2.0);
}

/// Full-width menu row: label on the left, shortcut on the right,
/// rounded highlight on hover. `danger` paints it red.
fn menu_item(ui: &mut Ui, text: &str, shortcut: &str, enabled: bool, danger: bool) -> Response {
    let pad = vec2(BUTTON_PADDING_X, BUTTON_PADDING_Y);
    let font = regular(14.0);
    let key_font = regular(12.5);
    let accent = if danger { RED } else { BLUE };
    let text_col = if !enabled { GRAY } else if danger { RED } else { TEXT };
    let key_col = if enabled { MUTED } else { Color32::from_rgb(200, 205, 214) };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, text_col);
    let key_galley = ui.painter().layout_no_wrap(shortcut.to_owned(), key_font, key_col);
    let gap = if shortcut.is_empty() { 0.0 } else { 32.0 };
    let want = galley.size().x + gap + key_galley.size().x + pad.x * 2.0;
    let h = galley.size().y.max(key_galley.size().y) + pad.y * 2.0;
    let w = ui.available_width().max(want);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), sense);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let t = ui.ctx().animate_bool_with_time(resp.id, enabled && resp.hovered(), 0.12);
        if t > 0.0 {
            let base = if danger { Color32::from_rgb(254, 236, 236) } else { BLUE_SOFT };
            let fill = if resp.is_pointer_button_down_on() {
                if danger { Color32::from_rgb(252, 222, 222) } else { Color32::from_rgb(218, 232, 255) }
            } else {
                base
            };
            p.rect_filled(rect, 8.0, fill.gamma_multiply(t));
            // accent bar on the left edge
            let bar = Rect::from_min_size(pos2(rect.left() + 3.0, rect.top() + 9.0), vec2(3.0, rect.height() - 18.0));
            p.rect_filled(bar, 1.5, accent.gamma_multiply(t));
        }
        let ty = rect.center().y - galley.size().y / 2.0;
        let col = if enabled && resp.hovered() && !danger { BLUE } else { text_col };
        p.galley_with_override_text_color(pos2(rect.left() + pad.x, ty), galley, col);
        if !shortcut.is_empty() {
            let kx = rect.right() - pad.x - key_galley.size().x;
            let ky = rect.center().y - key_galley.size().y / 2.0;
            p.galley(pos2(kx, ky), key_galley, key_col);
        }
    }
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Right-click menu for the empty area of the file list.
fn empty_menu(resp: Response, panel: &mut Panel, out: &mut PaneOut) {
    let is_pc = panel.is_pc();
    resp.context_menu(|ui| {
        menu_style(ui, 260.0);
        let mut item = |ui: &mut Ui, text: &str, key: &str, enabled: bool, a: Action| {
            let r = menu_item(ui, text, key, enabled, false);
            if r.clicked() {
                out.actions.push(a);
                ui.close_menu();
            }
        };
        item(ui, "Новая папка", "F7", !is_pc, Action::MkDir);
        item(ui, "Новый файл", "F9", !is_pc, Action::NewFile);
        item(ui, "Вставить", "Ctrl+V", !is_pc, Action::ClipboardPaste);
        menu_sep(ui);
        item(ui, "Размеры всех папок", "", !is_pc, Action::CalculateAllSizes);
        item(ui, "Отменить расчёт размеров", "", !panel.folder_sizes.pending.is_empty(), Action::CancelSizes);
        menu_sep(ui);
        item(ui, "Выделить всё", "Ctrl+A", true, Action::MarkAll);
        item(ui, "Снять выделение", "Esc", !panel.marked.is_empty(), Action::ClearMarks);
        menu_sep(ui);
        ui.menu_button(RichText::new("Сортировка").color(TEXT), |ui| {
            menu_style(ui, 210.0);
            for (label, key) in [
                ("По имени", SortKey::Name),
                ("По дате изменения", SortKey::Modified),
                ("По размеру", SortKey::Size),
                ("По типу", SortKey::Type),
            ] {
                let on = panel.sort == key;
                let arrow = if on { if panel.asc { "  ↑" } else { "  ↓" } } else { "" };
                let text = format!("{label}{arrow}");
                if menu_item(ui, &text, if on { "✓" } else { "" }, true, false).clicked() {
                    panel.set_sort(key);
                    ui.close_menu();
                }
            }
        });
        item(ui, "Найти файлы…", "Alt+F7", true, Action::FindFiles);
        item(ui, "Обновить", "Ctrl+R", true, Action::Refresh);
        item(ui, if panel.show_hidden { "Не показывать скрытые" } else { "Показывать скрытые" }, "F11", true, Action::ToggleHidden);
        menu_sep(ui);
        item(ui, "Добавить эту папку в избранное…", "", true, Action::AddCurrentFavorite);
        item(ui, "Открыть в Проводнике", "", !is_pc, Action::OpenInExplorer);
        item(ui, "Ввести путь", "Ctrl+L", true, Action::EditPath);
    });
}

/// Begin a rubber-band selection at screen y `press_y`.
fn start_band(panel: &mut Panel, press_y: f32, rows_r: Rect, keep: bool) {
    panel.selecting = true;
    panel.drag_base = if keep { panel.marked.clone() } else { Default::default() };
    panel.drag_start_y = (press_y - rows_r.top() + panel.scroll_offset).max(0.0);
}

/// Index into `Settings::col_widths` for a resizable header column.
fn column_index(label: &str) -> Option<usize> {
    match label {
        "Изменён" => Some(0),
        "Размер" => Some(1),
        "Тип" => Some(2),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct ColumnDrag {
    pointer_x: f32,
    width: f32,
    max_width: f32,
    visibility: (bool, bool),
}

impl ColumnDrag {
    fn width_at(self, pointer_x: f32) -> f32 {
        (self.width + self.pointer_x - pointer_x)
            .clamp(config::MIN_COL_WIDTH, self.max_width)
    }
}

fn pane_columns(ui: &Ui, idx: usize, head: Rect, widths: &[f32; 3]) -> Cols {
    let drag = ui.ctx().data(|d| d.get_temp::<ColumnDrag>(Id::new(("column-drag", idx))));
    match drag {
        Some(drag) => Cols::with_visibility(head, widths, Some(drag.visibility)),
        None => Cols::new(head, widths),
    }
}

/// Drag handles on the left edge of «Изменён», «Размер», «Тип»: dragging left widens
/// the column, «Имя» takes whatever is left. Double-click restores the default width.
fn resize_columns(ui: &mut Ui, idx: usize, head: Rect, widths: &mut [f32; 3], out: &mut PaneOut) {
    let cols = pane_columns(ui, idx, head, widths);
    let mut edges: Vec<(usize, f32)> = Vec::new();
    if let Some((x0, _)) = cols.modified {
        edges.push((0, x0));
    }
    edges.push((1, cols.size.0));
    if let Some((x0, _)) = cols.kind {
        edges.push((2, x0));
    }
    let name_w = cols.name.1 - cols.name.0;
    let mut hot = None;
    for (k, x) in edges {
        let grip = Rect::from_min_max(pos2(x - 5.0, head.top()), pos2(x + 5.0, head.bottom()));
        let resp = ui.interact(grip, Id::new(("col-grip", idx, k)), Sense::click_and_drag());
        if resp.hovered() || resp.dragged() {
            hot = Some(k);
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let drag_id = Id::new(("column-drag", idx));
        if resp.drag_started() {
            let pointer_x = ui.input(|i| i.pointer.press_origin()).map_or(x, |p| p.x);
            // Use the displayed width: the size column can be clipped in a narrow pane.
            let width = match k {
                0 => cols.modified.map(|c| c.1 - c.0).unwrap_or(widths[k]),
                1 => cols.size.1 - cols.size.0,
                _ => cols.kind.map(|c| c.1 - c.0).unwrap_or(widths[k]),
            };
            let drag = ColumnDrag {
                pointer_x,
                width,
                max_width: (width + (name_w - MIN_NAME_W).max(0.0)).max(config::MIN_COL_WIDTH),
                visibility: (cols.modified.is_some(), cols.kind.is_some()),
            };
            ui.ctx().data_mut(|d| d.insert_temp(drag_id, drag));
        }
        if resp.dragged() || resp.drag_stopped() {
            if let (Some(drag), Some(pointer)) = (
                ui.ctx().data(|d| d.get_temp::<ColumnDrag>(drag_id)),
                resp.interact_pointer_pos(),
            ) {
                widths[k] = drag.width_at(pointer.x);
            }
        }
        if resp.double_clicked() {
            widths[k] = config::DEFAULT_COL_WIDTHS[k];
            out.cols_changed = true;
        }
        if resp.drag_stopped() {
            ui.ctx().data_mut(|d| d.remove::<ColumnDrag>(drag_id));
            out.cols_changed = true;
        }
    }
    ui.ctx().memory_mut(|m| match hot {
        Some(k) => m.data.insert_temp(Id::new(("col-grip-hot", idx)), k),
        None => m.data.remove::<usize>(Id::new(("col-grip-hot", idx))),
    });
}

fn sort_arrow(p: &egui::Painter, at: egui::Pos2, asc: bool) {
    let (a, b, c) = if asc {
        (at + vec2(-4.5, 2.5), at + vec2(4.5, 2.5), at + vec2(0.0, -3.0))
    } else {
        (at + vec2(-4.5, -2.5), at + vec2(4.5, -2.5), at + vec2(0.0, 3.0))
    };
    p.add(egui::Shape::convex_polygon(vec![a, b, c], TEXT_SOFT, Stroke::NONE));
}

fn table(ui: &mut Ui, panel: &mut Panel, active: bool, idx: usize, rect: Rect, col_widths: &mut [f32; 3], out: &mut PaneOut, mut file_icons: Option<&mut crate::file_icons::FileIcons>) {
    let opening = ui.ctx().data(|d| d.get_temp::<OpenFeedback>(open_feedback_id()));
    let now = ui.input(|i| i.time);
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), HEAD_H));
    resize_columns(ui, idx, head, col_widths, out);
    let cols = pane_columns(ui, idx, head, col_widths);
    let p = ui.painter().clone();
    p.rect_filled(head, 8.0, HEADER_BG);

    let mut header_cols: Vec<(&str, SortKey, (f32, f32))> = vec![("Имя", SortKey::Name, cols.name)];
    let grip_hot = |k: usize| ui.ctx().memory(|m| m.data.get_temp::<usize>(Id::new(("col-grip-hot", idx)))) == Some(k);
    if let Some(c) = cols.modified {
        header_cols.push(("Изменён", SortKey::Modified, c));
    }
    header_cols.push(("Размер", SortKey::Size, cols.size));
    if let Some(c) = cols.kind {
        header_cols.push(("Тип", SortKey::Type, c));
    }
    for (k, (label, key, (x0, x1))) in header_cols.into_iter().enumerate() {
        // leave the column edges to the resize grips (see `resize_columns`)
        let cr = Rect::from_min_max(pos2(x0 + 5.0, head.top()), pos2(x1 - 5.0, head.bottom()));
        let resp = ui.interact(cr, Id::new(("head", idx, k)), Sense::click());
        if k > 0 {
            let col = column_index(label);
            let (h, c) = if col.is_some_and(grip_hot) { (13.0, BLUE) } else { (9.0, BORDER) };
            p.line_segment([pos2(x0, head.center().y - h), pos2(x0, head.center().y + h)], Stroke::new(if c == BLUE { 2.0 } else { 1.0 }, c));
        }
        let tx = if k == 0 { x0 + 20.0 } else { x0 + 14.0 };
        let r = p.text(pos2(tx, head.center().y), Align2::LEFT_CENTER, label, regular(12.5), if resp.hovered() { BLUE } else { TEXT_SOFT });
        if panel.sort == key {
            sort_arrow(&p, pos2(r.right() + 10.0, head.center().y), panel.asc);
        }
        if resp.clicked() {
            panel.set_sort(key);
        }
    }

    let rows_r = Rect::from_min_max(pos2(rect.left(), head.bottom() + 4.0), rect.max);
    out.zone.folder = if panel.results.is_some() { None } else { Some(panel.path.clone()).filter(|p| !p.as_os_str().is_empty()) };
    let n = panel.visible.len();
    if n == 0 || panel.error.is_some() {
        let msg = match &panel.error {
            Some(e) => format!("Не удалось прочитать папку: {e}"),
            None if panel.filter_error.is_some() => panel.filter_error.clone().unwrap_or_default(),
            None if !panel.filter.is_empty() => "Ничего не найдено".into(),
            None => "Папка пуста".into(),
        };
        p.text(
            pos2(rows_r.center().x, rows_r.top() + 60.0),
            Align2::CENTER_CENTER,
            msg,
            regular(12.5),
            if panel.error.is_some() || panel.filter_error.is_some() { RED } else { MUTED },
        );
        if n == 0 {
            let empty = ui.interact(rows_r, Id::new(("rows-bg", idx)), Sense::click());
            empty_menu(empty, panel, out);
            return;
        }
    }

    let mut sa = egui::ScrollArea::vertical().id_salt(("rows", idx)).auto_shrink([false, false]).drag_to_scroll(false);

    // ── rubber-band selection: press on empty space (or on a row) and drag ──
    let (ctrl, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
    let (primary_down, pointer, press) =
        ui.input(|i| (i.pointer.primary_down(), i.pointer.interact_pos(), i.pointer.press_origin()));
    // background catcher: registered before the rows, so rows win where they exist
    // and this one gets the empty area below the last row.
    let empty = ui.interact(rows_r, Id::new(("rows-bg", idx)), Sense::click_and_drag());
    if empty.drag_started_by(egui::PointerButton::Primary) {
        start_band(panel, press.map_or(rows_r.top(), |p| p.y), rows_r, ctrl);
    }
    if empty.clicked() && !ctrl && !shift {
        panel.marked.clear();
    }
    empty_menu(empty, panel, out);
    let mut band: Option<(f32, f32)> = None;
    if panel.selecting {
        match pointer {
            Some(pos) if primary_down => {
                // auto-scroll when the pointer leaves the list vertically
                let max_off = (n as f32 * ROW_H - panel.view_height).max(0.0);
                let mut off = panel.scroll_offset;
                if pos.y < rows_r.top() + 6.0 {
                    off -= ((rows_r.top() + 6.0 - pos.y) * 0.4).clamp(3.0, 40.0);
                } else if pos.y > rows_r.bottom() - 6.0 {
                    off += ((pos.y - rows_r.bottom() + 6.0) * 0.4).clamp(3.0, 40.0);
                }
                let off = off.clamp(0.0, max_off);
                if off != panel.scroll_offset {
                    sa = sa.vertical_scroll_offset(off);
                    panel.scroll_offset = off;
                }
                ui.ctx().request_repaint();
                let cur_y = (pos.y.clamp(rows_r.top(), rows_r.bottom()) - rows_r.top() + off).max(0.0);
                let (y0, y1) = (panel.drag_start_y.min(cur_y), panel.drag_start_y.max(cur_y));
                panel.marked = panel.drag_base.clone();
                let first = (y0 / ROW_H) as usize;
                if first < n {
                    let last = ((y1 / ROW_H) as usize).min(n - 1);
                    panel.mark_range(first, last);
                    panel.cursor = ((cur_y / ROW_H) as usize).min(n - 1);
                }
                band = Some((panel.drag_start_y - off + rows_r.top(), cur_y - off + rows_r.top()));
            }
            _ => panel.selecting = false,
        }
    }

    if panel.ensure_visible && !panel.selecting {
        let y = panel.cursor as f32 * ROW_H;
        let mut off = panel.scroll_offset;
        if y < off {
            off = y;
        } else if y + ROW_H > off + panel.view_height {
            off = y + ROW_H - panel.view_height;
        }
        sa = sa.vertical_scroll_offset(off.max(0.0));
        panel.ensure_visible = false;
    }
    #[allow(deprecated)]
    let sa_out = ui
        .allocate_ui_at_rect(rows_r, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            sa.show_rows(ui, ROW_H, n, |ui, range| {
                for i in range {
                    let e = panel.all[panel.visible[i]].clone();
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
                    if e.is_dir {
                        out.zone.rows.push((rect.intersect(rows_r), e.path.clone()));
                    }
                    let pr = ui.painter();
                    let is_cur = i == panel.cursor;
                    let is_marked = panel.marked.contains(&e.path);
                    let bg = rect.shrink2(vec2(0.0, 1.5));
                    if is_cur {
                        pr.rect_filled(bg, 8.0, if active { SEL_ACTIVE } else { SEL_INACTIVE });
                    } else if is_marked {
                        pr.rect_filled(bg, 8.0, MARKED_BG);
                    } else if resp.hovered() {
                        pr.rect_filled(bg, 8.0, HOVER);
                    }
                    let open_age = opening.as_ref().filter(|o| o.path == e.path)
                        .map(|o| (now - o.started).max(0.0) as f32)
                        .filter(|age| *age < OPEN_FEEDBACK_SECS as f32);
                    let open_fade = open_age.map_or(0.0, |age| {
                        ((OPEN_FEEDBACK_SECS as f32 - age) / 0.3).clamp(0.0, 1.0)
                    });
                    if let Some(age) = open_age {
                        let pulse = (1.0 - age / OPEN_FEEDBACK_SECS as f32).powi(2);
                        pr.rect_filled(bg, 8.0, BLUE.gamma_multiply(0.16 * pulse));
                        let stripe = Rect::from_min_size(bg.min + vec2(2.0, 5.0), vec2(3.0, bg.height() - 10.0));
                        pr.rect_filled(stripe, 2.0, BLUE.gamma_multiply(open_fade));
                    }
                    if !is_cur {
                        pr.line_segment(
                            [pos2(rect.left() + 10.0, rect.bottom() - 0.5), pos2(rect.right() - 10.0, rect.bottom() - 0.5)],
                            Stroke::new(1.0, ROW_LINE),
                        );
                    }
                    let cy = rect.center().y;
                    let bounce = open_age.map_or(0.0, |age| {
                        ((age / 0.32).clamp(0.0, 1.0) * std::f32::consts::PI).sin() * 3.0
                    });
                    let ir = Rect::from_center_size(pos2(cols.name.0 + 34.0, cy), vec2(26.0 + bounce, 26.0 + bounce));
                    if e.is_parent {
                        icons::parent(pr, ir);
                    } else if e.is_drive {
                        icons::drive(pr, ir);
                    } else if e.is_dir {
                        icons::folder(pr, ir);
                    } else {
                        crate::file_icons::paint(file_icons.as_deref_mut(), pr, ir, &e.path, e.kind, &e.ext);
                    }
                    let show_open_status = open_age.is_some() && cols.name.1 - cols.name.0 >= 300.0;
                    let status_width = if show_open_status { 110.0 } else { 0.0 };
                    let name_clip = Rect::from_min_max(pos2(cols.name.0, rect.top()), pos2(cols.name.1 - 8.0 - status_width, rect.bottom()));
                    if let Some(age) = open_age {
                        let status = pr.with_clip_rect(Rect::from_min_max(
                            pos2(cols.name.0, rect.top()), pos2(cols.name.1 - 8.0, rect.bottom()),
                        ));
                        let center = if show_open_status { pos2(cols.name.1 - 107.0, cy) } else { ir.center() };
                        let radius = if show_open_status { 5.0 } else { 17.0 };
                        let points = (0..=20).map(|step| {
                            let angle = age * 8.0 + step as f32 / 20.0 * std::f32::consts::PI * 1.5;
                            center + vec2(angle.cos(), angle.sin()) * radius
                        }).collect();
                        status.add(egui::Shape::line(points, Stroke::new(1.7, BLUE.gamma_multiply(open_fade))));
                        if show_open_status {
                            status.text(pos2(center.x + 11.0, cy), Align2::LEFT_CENTER,
                                "Открывается…", regular(11.0), BLUE.gamma_multiply(open_fade));
                        }
                    }
                    let name_color = if is_marked {
                        MARKED_FG
                    } else if e.hidden {
                        MUTED
                    } else {
                        TEXT
                    };
                    let label = if e.is_drive { format!("Локальный диск ({})", e.name) } else { e.name.clone() };
                    let text_r = pr.with_clip_rect(name_clip).text(
                        pos2(cols.name.0 + 58.0, cy),
                        Align2::LEFT_CENTER,
                        label,
                        if is_marked { semibold(13.0) } else { regular(13.0) },
                        name_color,
                    );
                    if resp.drag_started_by(egui::PointerButton::Primary) {
                        // by the icon or the name: drag the files; elsewhere: selection band
                        let grip = ir.union(text_r).expand2(vec2(6.0, 4.0)).intersect(name_clip);
                        let draggable = !e.is_parent && !e.is_drive && !panel.selecting;
                        if draggable && press.is_some_and(|p| grip.contains(p)) {
                            let sources = if panel.marked.contains(&e.path) { panel.targets() } else { vec![e.path.clone()] };
                            panel.cursor = i;
                            out.drag_start = Some(sources);
                        } else {
                            start_band(panel, press.map_or(rect.center().y, |p| p.y), rows_r, ctrl);
                        }
                    }
                    if let Some((x0, x1)) = cols.modified {
                        let t = if e.is_parent || e.is_drive { String::new() } else { format_time(e.modified) };
                        pr.with_clip_rect(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom())))
                            .text(pos2(x0 + 14.0, cy), Align2::LEFT_CENTER, t, regular(12.0), TEXT_SOFT);
                    }
                    let size_txt = if e.is_parent {
                        String::new()
                    } else if e.is_dir && !e.is_drive {
                        if panel.folder_sizes.pending.contains(&e.path) {
                            "…".into()
                        } else if let Some(size) = panel.folder_sizes.sizes.get(&e.path) {
                            if size.skipped > 0 { format!("≥ {}", format_size(size.bytes)) }
                            else { format_size(size.bytes) }
                        } else { "—".into() }
                    } else {
                        format_size(e.size)
                    };
                    pr.text(pos2(cols.size.1 - 16.0, cy), Align2::RIGHT_CENTER, size_txt, regular(12.0), TEXT_SOFT);
                    if e.is_dir && !e.is_parent && !e.is_drive {
                        let cell = Rect::from_min_max(pos2(cols.size.0, rect.top()), pos2(cols.size.1, rect.bottom()));
                        if let Some(size) = panel.folder_sizes.sizes.get(&e.path) {
                            let hint = if size.skipped > 0 {
                                format!("Неполный размер: {} байт. Пропущено недоступных объектов или ссылок: {}", size.bytes, size.skipped)
                            } else { format!("{} байт, включая вложенные и скрытые файлы", size.bytes) };
                            if ui.rect_contains_pointer(cell) { resp.clone().on_hover_text(hint); }
                        }
                    }
                    if let Some((x0, x1)) = cols.kind {
                        pr.with_clip_rect(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1 - 4.0, rect.bottom())))
                            .text(pos2(x0 + 14.0, cy), Align2::LEFT_CENTER, &e.type_name, regular(12.0), TEXT_SOFT);
                    }

                    if resp.clicked() || resp.secondary_clicked() {
                        if resp.clicked() && ctrl {
                            panel.toggle_mark(i);
                        } else if resp.clicked() && shift {
                            let c = panel.cursor;
                            panel.mark_range(c, i);
                        }
                        panel.cursor = i;
                    }
                    if row_double_clicked(&resp, Id::new(("file-row", idx, &panel.path, &e.path))) {
                        panel.cursor = i;
                        out.actions.push(Action::Open);
                    }
                    resp.context_menu(|ui| {
                        menu_style(ui, 280.0);
                        let fav = if e.is_dir && !e.is_parent {
                            "Добавить в избранное…\tCtrl+D"
                        } else {
                            "Добавить эту папку в избранное…\tCtrl+D"
                        };
                        let mut items: Vec<(&str, Action)> = vec![
                            ("Открыть\tEnter", Action::Open),
                            ("Просмотр\tF3", Action::View),
                            ("Редактировать\tF4", Action::Edit),
                            ("Встроенный редактор\tShift+F4", Action::EditBuiltin),
                            ("Переименовать\tF2", Action::Rename),
                            ("Копировать\tCtrl+C", Action::ClipboardCopy),
                            ("Вырезать\tCtrl+X", Action::ClipboardCut),
                            ("Вставить\tCtrl+V", Action::ClipboardPaste),
                            ("Копировать в другую панель\tF5", Action::Copy),
                            ("Переместить в другую панель\tF6", Action::Move),
                            ("Добавить в ZIP-архив…\tAlt+F5", Action::Zip),
                            (fav, Action::AddFavorite),
                            ("Удалить в корзину\tDel", Action::Delete),
                            ("Удалить навсегда\tShift+Del", Action::DeletePermanent),
                            ("Новая папка\tF7", Action::MkDir),
                            ("Новый файл\tF9", Action::NewFile),
                            ("Отметить / снять\tSpace", Action::Mark),
                            ("Отметить всё\tCtrl+A", Action::MarkAll),
                        ];
                        if is_zip(&e.path) {
                            items.insert(10, ("Распаковать…\tAlt+F6", Action::Unzip));
                        }
                        if e.is_dir && !e.is_parent && !e.is_drive {
                            items.insert(1, ("Рассчитать размер папок\tAlt+Enter", Action::CalculateSizes));
                        }
                        items.push(("Размеры всех папок", Action::CalculateAllSizes));
                        if !panel.folder_sizes.pending.is_empty() {
                            items.push(("Отменить расчёт размеров", Action::CancelSizes));
                        }
                        for (label, a) in items {
                            if matches!(a, Action::Zip | Action::Delete | Action::MkDir | Action::Mark) {
                                menu_sep(ui);
                            }
                            let (name, key) = label.split_once('\t').unwrap_or((label, ""));
                            let r = menu_item(ui, name, key, true, matches!(a, Action::Delete | Action::DeletePermanent));
                            if r.clicked() {
                                out.actions.push(a);
                                ui.close_menu();
                            }
                        }
                    });
                }
            })
        })
        .inner;
    panel.scroll_offset = sa_out.state.offset.y;
    panel.view_height = sa_out.inner_rect.height();

    // translucent selection band while dragging
    if let Some((y0, y1)) = band {
        let r = Rect::from_min_max(pos2(rows_r.left() + 2.0, y0.min(y1)), pos2(rows_r.right() - 2.0, y0.max(y1)));
        let p = ui.painter().with_clip_rect(rows_r);
        p.rect(r, 6.0, Color32::from_rgba_unmultiplied(22, 119, 255, 28), Stroke::new(1.0, Color32::from_rgba_unmultiplied(22, 119, 255, 140)));
    }
}

fn footer(ui: &mut Ui, panel: &Panel, rect: Rect) {
    let p = ui.painter().clone();
    p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, BORDER));
    let cy = rect.center().y;
    icons::folder(&p, Rect::from_center_size(pos2(rect.left() + 26.0, cy), vec2(20.0, 18.0)));
    let items = panel.visible.iter().filter(|&&i| !panel.all[i].is_parent).count();
    let mut x = p.text(pos2(rect.left() + 44.0, cy), Align2::LEFT_CENTER, items_text(items as u64), regular(14.0), TEXT_SOFT).right();
    if !panel.marked.is_empty() {
        p.line_segment([pos2(x + 12.0, cy - 9.0), pos2(x + 12.0, cy + 9.0)], Stroke::new(1.0, BORDER));
        x = p
            .text(
                pos2(x + 24.0, cy),
                Align2::LEFT_CENTER,
                format!("Выбрано: {} ({})", panel.marked.len(), format_size(panel.marked_size())),
                regular(14.0),
                MARKED_FG,
            )
            .right();
    }
    let _ = x;
    let mut rx = rect.right() - 16.0;
    if !panel.is_pc() {
        let r = p.text(pos2(rx, cy), Align2::RIGHT_CENTER, format!("Всего {}", format_size(panel.bytes)), regular(14.0), TEXT_SOFT);
        rx = r.left() - 12.0;
        if let Some((total, free)) = panel.disk {
            p.line_segment([pos2(rx, cy - 9.0), pos2(rx, cy + 9.0)], Stroke::new(1.0, BORDER));
            let used = 1.0 - free as f32 / total.max(1) as f32;
            let br = Rect::from_min_size(pos2(rx - 12.0 - 60.0, cy - 3.0), vec2(60.0, 6.0));
            icons::bar(&p, br, used, if used > 0.9 { RED } else { BLUE });
            p.text(pos2(br.left() - 8.0, cy), Align2::RIGHT_CENTER, format!("Свободно {}", format_size(free)), regular(14.0), TEXT_SOFT);
        }
    }
}

// ───────────────────────────── widgets ─────────────────────────────

fn vsep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(9.0, 30.0), Sense::hover());
    ui.painter().line_segment([pos2(r.center().x, r.top()), pos2(r.center().x, r.bottom())], Stroke::new(1.0, BORDER));
}

fn top_btn(ui: &mut Ui, g: Glyph, color: Color32, enabled: bool, on: bool, bordered: bool) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let fill = if on {
        BLUE_SOFT
    } else if resp.hovered() && enabled {
        HOVER
    } else {
        CARD
    };
    p.rect(r, 10.0, fill, if bordered { Stroke::new(1.0, BORDER) } else { Stroke::NONE });
    icons::glyph(p, r.shrink(10.0), g, if enabled { color } else { Color32::from_rgb(200, 206, 216) });
    resp
}

/// Sidebar entry: a square icon button, or a full-width row with a caption.
fn side_btn(ui: &mut Ui, look: Look, active: bool, label: Option<&str>, width: f32) -> Response {
    let size = match label {
        Some(_) => vec2(width, 38.0),
        None => vec2(44.0, 44.0),
    };
    let (r, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let p = ui.painter();
    if active {
        p.rect_filled(r, 10.0, BLUE_SOFT);
    } else if resp.hovered() {
        p.rect_filled(r, 10.0, HOVER);
    }
    let color = if active { BLUE } else { SLATE };
    match label {
        Some(text) => {
            let ir = Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), vec2(22.0, 22.0));
            draw_look(p, ir, look, color);
            p.with_clip_rect(r.shrink(2.0)).text(
                pos2(r.left() + 40.0, r.center().y),
                Align2::LEFT_CENTER,
                text,
                if active { semibold(14.0) } else { regular(14.0) },
                if active { BLUE } else { TEXT },
            );
        }
        None => draw_look(p, r.shrink(11.0), look, color),
    }
    resp
}

/// Grid of built-in icons; returns true when one was clicked.
fn icon_picker(ui: &mut Ui, current: &mut Glyph, tint: Option<Color32>, selected: bool) -> bool {
    let mut clicked = false;
    let per_row = 11;
    for row in Glyph::PICKABLE.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (g, _) in row {
                let (r, resp) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::click());
                let on = selected && *current == *g;
                let p = ui.painter();
                if on {
                    p.rect(r, 8.0, CARD, Stroke::new(1.5, BLUE));
                } else if resp.hovered() {
                    p.rect_filled(r, 8.0, CARD);
                }
                icons::glyph(p, r.shrink(8.0), *g, tint.unwrap_or(if on { BLUE } else { SLATE }));
                if resp.clicked() {
                    *current = *g;
                    clicked = true;
                }
            }
        });
    }
    clicked
}

/// Paged grid with every icon of `ed.lib` (a dll / exe / ico).
fn resource_grid(ui: &mut Ui, cache: &mut IconCache, ctx: &egui::Context, ed: &mut PlaceEditor) {
    const COLS: usize = 12;
    const ROWS: usize = 5;
    let per_page = COLS * ROWS;
    let total = *ed.lib_count.get_or_insert_with(|| iconsrc::count(&ed.lib));
    if total == 0 {
        ui.label(RichText::new("В этом файле нет значков").color(MUTED));
        return;
    }
    let pages = (total + per_page - 1) / per_page;
    ed.page = ed.page.min(pages - 1);
    let start = ed.page * per_page;
    let chosen = ed.icon.as_deref().map(iconsrc::split_spec);
    let folder = PathBuf::new();
    for row in 0..ROWS {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for col in 0..COLS {
                let i = start + row * COLS + col;
                if i >= total {
                    return;
                }
                let (r, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
                let on = chosen.as_ref().is_some_and(|(f, n)| *n == i as i32 && f == &ed.lib);
                let p = ui.painter();
                if on {
                    p.rect(r, 8.0, CARD, Stroke::new(1.5, BLUE));
                } else if resp.hovered() {
                    p.rect_filled(r, 8.0, CARD);
                }
                let spec = iconsrc::make_spec(&ed.lib, i as i32);
                if let Some(t) = icon_texture(cache, ctx, &spec, &folder, 32) {
                    p.image(t, r.shrink(6.0), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                if resp.on_hover_text(format!("Значок №{i}")).clicked() {
                    ed.icon = Some(spec);
                }
            }
        });
    }
    if pages > 1 {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(ed.page > 0, egui::Button::new("◀")).clicked() {
                ed.page -= 1;
            }
            ui.label(RichText::new(format!("Страница {} из {} · всего значков: {total}", ed.page + 1, pages)).size(12.5).color(TEXT_SOFT));
            if ui.add_enabled(ed.page + 1 < pages, egui::Button::new("▶")).clicked() {
                ed.page += 1;
            }
        });
    }
}

/// Small rounded toggle button.
fn pill(ui: &mut Ui, text: &str, on: bool) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(13.5).color(if on { BLUE } else { TEXT }))
            .fill(if on { BLUE_SOFT } else { INPUT_BG })
            .stroke(if on { Stroke::new(1.0, BLUE) } else { Stroke::new(1.0, BORDER) })
            .rounding(16.0)
            .min_size(vec2(0.0, 30.0)),
    )
}

/// Draw a favourite's icon: custom texture, or the glyph in its colour (`fallback` otherwise).
fn draw_look(p: &egui::Painter, r: Rect, look: Look, fallback: Color32) {
    match look.tex {
        Some(t) => {
            p.image(t, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => icons::glyph(p, r, look.glyph, look.tint.unwrap_or(fallback)),
    }
}

/// Left-hand list of settings groups.
fn settings_nav(ui: &mut Ui, group: &mut SettingsGroup) {
    ui.vertical(|ui| {
        ui.set_width(196.0);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(RichText::new("ПАРАМЕТРЫ").font(semibold(10.5)).color(MUTED));
        ui.add_space(6.0);
        for (g, name, glyph) in SettingsGroup::ALL {
            let (r, resp) = ui.allocate_exact_size(vec2(196.0, 44.0), Sense::click());
            let on = *group == g;
            let p = ui.painter();
            if on {
                p.rect_filled(r, 10.0, BLUE_SOFT);
            } else if resp.hovered() {
                p.rect_filled(r, 10.0, HOVER);
            }
            icons::glyph(p, Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), vec2(20.0, 20.0)), glyph, if on { BLUE } else { SLATE });
            p.text(pos2(r.left() + 40.0, r.center().y), Align2::LEFT_CENTER, name, if on { semibold(13.0) } else { regular(13.0) }, if on { BLUE } else { TEXT_SOFT });
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, name));
            if resp.clicked() {
                *group = g;
            }
        }
    });
}

fn section_title(ui: &mut Ui, title: &str, hint: Option<&str>) {
    ui.label(RichText::new(title).font(semibold(15.0)).color(TEXT));
    if let Some(h) = hint {
        ui.add(egui::Label::new(RichText::new(h).size(12.0).color(TEXT_SOFT)).wrap());
    }
}

/// A padded setting card with a wrapping explanation.
fn option(ui: &mut Ui, value: &mut bool, label: &str, hint: &str) -> bool {
    egui::Frame::none()
        .fill(HOVER)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(12.0)
        .inner_margin(Margin::same(14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            let changed = ui.checkbox(value, RichText::new(label).font(semibold(13.0)).color(TEXT)).changed();
            ui.add(egui::Label::new(RichText::new(hint).size(12.0).color(TEXT_SOFT)).wrap());
            changed
        }).inner
}

fn kbd(ui: &mut Ui, k: &str) {
    let g = ui.painter().layout_no_wrap(k.to_string(), semibold(12.5), BLUE);
    let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 16.0, 24.0), Sense::hover());
    ui.painter().rect_filled(r, 6.0, BLUE_SOFT);
    ui.painter().galley(r.center() - g.size() / 2.0, g, BLUE);
}

fn btn(ui: &mut Ui, text: &str, fill: Color32, fg: Color32) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).font(semibold(14.0)).color(fg))
            .fill(fill)
            .stroke(Stroke::NONE)
            .rounding(10.0)
            .min_size(vec2(0.0, 38.0)),
    )
}

fn buttons_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 44.0), Layout::right_to_left(Align::Center), add);
}

fn dialog_header(ui: &mut Ui, g: Glyph, color: Color32, title: &str, subtitle: Option<&str>) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
        ui.painter().rect_filled(r, 12.0, mix(color, Color32::WHITE, 0.86));
        icons::glyph(ui.painter(), r.shrink(11.0), g, color);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).font(semibold(18.0)).color(TEXT));
            if let Some(s) = subtitle {
                ui.label(RichText::new(s).size(13.0).color(MUTED));
            }
        });
    });
    ui.add_space(14.0);
}

// ───────────────────────────── operation queue ─────────────────────────────

enum QueueOp {
    Open,
    Close,
    Pause,
    ClearFinished,
    Cancel(u64),
    Retry(u64),
    Remove(u64),
    Up(u64),
    Down(u64),
}

fn is_zip(p: &std::path::Path) -> bool {
    p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

fn file_stem_of(p: &std::path::Path) -> String {
    p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Архив".into())
}

fn kind_look(k: OpKind) -> (Glyph, Color32) {
    match k {
        OpKind::Copy => (Glyph::Copy, BLUE),
        OpKind::Move => (Glyph::ArrowRight, GREEN),
        OpKind::Delete => (Glyph::Trash, RED),
        OpKind::Archive => (Glyph::Archive, ORANGE),
    }
}

fn icon_btn(ui: &mut Ui, g: Glyph, color: Color32, tip: &str) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 6.0, INPUT_BG);
    }
    icons::glyph(ui.painter(), r.shrink(6.0), g, color);
    resp.on_hover_text(tip)
}

fn small_btn(ui: &mut Ui, text: &str, fg: Color32) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).font(semibold(12.5)).color(fg))
            .fill(INPUT_BG)
            .stroke(Stroke::NONE)
            .rounding(7.0)
            .min_size(vec2(0.0, 26.0)),
    )
}

fn progress_text(t: &Task) -> String {
    let State::Running(job) = &t.state else { return String::new() };
    if job.total == 0 {
        "Подготовка…".into()
    } else if job.bytes {
        format!("{} из {}", format_size(job.done), format_size(job.total))
    } else {
        format!("{} из {}", job.done, job.total)
    }
}

/// One line of the queue: kind icon, title, status / progress and per-task buttons.
fn queue_row(ui: &mut Ui, t: &Task, first_pending: bool, last_pending: bool, ops: &mut Vec<QueueOp>) {
    let (g, color) = kind_look(t.spec.kind());
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
        ui.painter().rect_filled(r, 9.0, mix(color, Color32::WHITE, 0.86));
        icons::glyph(ui.painter(), r.shrink(8.0), g, color);
        let buttons_w = 64.0;
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - buttons_w);
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.add(egui::Label::new(RichText::new(t.title.as_str()).color(TEXT).size(13.5)).truncate());
            let (status, col) = match &t.state {
                State::Pending => ("Ожидает".to_string(), MUTED),
                State::Running(job) => {
                    let (br, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
                    icons::bar(ui.painter(), br, job.fraction(), color);
                    let cur = if job.current.is_empty() { String::new() } else { format!(" · {}", job.current) };
                    (format!("{}{cur}", progress_text(t)), TEXT_SOFT)
                }
                State::Done(m) => (m.clone(), GREEN),
                State::Failed(e) => (e.clone(), RED),
                State::Cancelled => ("Отменено".to_string(), MUTED),
            };
            ui.add(egui::Label::new(RichText::new(status).color(col).size(12.0)).truncate());
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            match &t.state {
                State::Running(_) => {
                    if icon_btn(ui, Glyph::Close, RED, "Отменить").clicked() {
                        ops.push(QueueOp::Cancel(t.id));
                    }
                }
                State::Pending => {
                    if icon_btn(ui, Glyph::Close, TEXT_SOFT, "Убрать из очереди").clicked() {
                        ops.push(QueueOp::Cancel(t.id));
                    }
                    if !last_pending && icon_btn(ui, Glyph::ChevronDown, TEXT_SOFT, "Позже").clicked() {
                        ops.push(QueueOp::Down(t.id));
                    }
                    if !first_pending && icon_btn(ui, Glyph::ArrowUp, TEXT_SOFT, "Раньше").clicked() {
                        ops.push(QueueOp::Up(t.id));
                    }
                }
                State::Done(_) => {
                    if icon_btn(ui, Glyph::Close, MUTED, "Убрать из списка").clicked() {
                        ops.push(QueueOp::Remove(t.id));
                    }
                }
                State::Failed(_) | State::Cancelled => {
                    if icon_btn(ui, Glyph::Close, MUTED, "Убрать из списка").clicked() {
                        ops.push(QueueOp::Remove(t.id));
                    }
                    if icon_btn(ui, Glyph::Refresh, BLUE, "Повторить").clicked() {
                        ops.push(QueueOp::Retry(t.id));
                    }
                }
            }
        });
    });
}

fn queue_full(ui: &mut Ui, q: &Queue, ops: &mut Vec<QueueOp>) {
    let pending = q.pending_count();
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new("Очередь операций").font(semibold(16.0)).color(TEXT));
            let sub = match (q.running().is_some(), pending, q.paused) {
                (false, 0, _) => "Нет активных операций".to_string(),
                (_, n, true) => format!("Пауза · ожидает: {n}"),
                (true, 0, _) => "Выполняется 1 операция".to_string(),
                (_, n, _) => format!("Ожидает: {n}"),
            };
            ui.label(RichText::new(sub).size(12.5).color(MUTED));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_btn(ui, Glyph::ChevronDown, TEXT_SOFT, "Свернуть (Ctrl+J)").clicked() {
                ops.push(QueueOp::Close);
            }
            if q.paused || pending > 0 {
                let (label, fg) = if q.paused { ("Продолжить", BLUE) } else { ("Пауза", TEXT_SOFT) };
                if small_btn(ui, label, fg).on_hover_text("Текущая операция доработает, следующие не начнутся").clicked() {
                    ops.push(QueueOp::Pause);
                }
            }
        });
    });
    ui.add_space(8.0);
    if q.tasks.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new("Копирование, перемещение, удаление и архивы попадают сюда и выполняются по очереди.").size(12.5).color(MUTED));
        return;
    }
    let pend_ids: Vec<u64> = q.tasks.iter().filter(|t| matches!(t.state, State::Pending)).map(|t| t.id).collect();
    egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        // running and waiting first, then the history (newest on top)
        let live = q.tasks.iter().filter(|t| !t.is_finished());
        let done = q.tasks.iter().rev().filter(|t| t.is_finished());
        for t in live.chain(done) {
            queue_row(ui, t, pend_ids.first() == Some(&t.id), pend_ids.last() == Some(&t.id), ops);
        }
    });
    if q.has_finished() {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if small_btn(ui, "Очистить завершённые", TEXT_SOFT).clicked() {
                ops.push(QueueOp::ClearFinished);
            }
        });
    }
}

fn queue_compact(ui: &mut Ui, q: &Queue, ops: &mut Vec<QueueOp>) {
    let Some(t) = q.running().or_else(|| q.tasks.iter().find(|t| matches!(t.state, State::Pending))) else { return };
    queue_row(ui, t, true, true, ops);
    let more = q.pending_count() - usize::from(matches!(t.state, State::Pending));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let note = match (more, q.paused) {
            (_, true) => "Очередь на паузе".to_string(),
            (0, _) => String::new(),
            (n, _) => format!("Ещё в очереди: {n}"),
        };
        ui.label(RichText::new(note).size(12.0).color(MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if small_btn(ui, "Вся очередь", BLUE).on_hover_text("Ctrl+J").clicked() {
                ops.push(QueueOp::Open);
            }
        });
    });
}

// ───────────────────────────── animation ─────────────────────────────

/// Duration of the dialog "pop" animation, seconds.
const DIALOG_ANIM: f64 = 0.32;
/// Duration of the backdrop fade, seconds.
const DIM_ANIM: f32 = 0.22;

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Ease-out with a small overshoot, like the macOS alert "pop".
fn ease_out_back(t: f32) -> f32 {
    const C1: f32 = 1.35;
    const C3: f32 = C1 + 1.0;
    1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
}

/// 0 → 1 progress of an appearance animation for `key`. Restarts whenever the
/// element was not shown on the previous frame, so every opening animates.
fn appear_progress(ctx: &egui::Context, key: Id, duration: f64) -> f32 {
    let now = ctx.input(|i| i.time);
    let pass = ctx.cumulative_pass_nr();
    let start = ctx.data_mut(|d| {
        let st = d.get_temp_mut_or_insert_with(key, || (now, pass));
        if st.1 + 1 < pass {
            st.0 = now;
        }
        st.1 = pass;
        st.0
    });
    let t = ((now - start) / duration).clamp(0.0, 1.0) as f32;
    if t < 1.0 {
        ctx.request_repaint();
    }
    t
}

/// Darkened backdrop behind dialogs; fades in when one opens and out when the last one closes.
fn backdrop(ctx: &egui::Context, open: bool) {
    let t = ctx.animate_bool_with_time(Id::new("modal-dim-anim"), open, DIM_ANIM);
    if t <= 0.0 {
        return;
    }
    let screen = ctx.screen_rect();
    let alpha = (ease_out_cubic(t) * 90.0) as u8;
    egui::Area::new(Id::new("modal-dim"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .movable(false)
        .fade_in(false)
        .interactable(open)
        .show(ctx, |ui| {
            ui.allocate_exact_size(screen.size(), if open { Sense::click() } else { Sense::hover() });
            ui.painter().rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(15, 23, 42, alpha));
        });
}

fn dialog<R>(ctx: &egui::Context, id: &str, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let screen = ctx.screen_rect();
    let t = appear_progress(ctx, Id::new(("dialog-anim", id)), DIALOG_ANIM);
    // fade finishes a bit earlier than the scale, which settles after a tiny overshoot
    let opacity = ease_out_cubic((t * 1.7).min(1.0));
    let scale = 0.92 + 0.08 * ease_out_back(t);
    let lift = -14.0 * (1.0 - ease_out_cubic(t));
    let c = screen.center().to_vec2();
    let transform = egui::emath::TSTransform::from_translation(c + vec2(0.0, lift))
        * egui::emath::TSTransform::from_scaling(scale)
        * egui::emath::TSTransform::from_translation(-c);
    let layer = egui::LayerId::new(egui::Order::Foreground, Id::new(id));
    ctx.set_transform_layer(layer, transform);
    egui::Area::new(Id::new(id))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .movable(false)
        .fade_in(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(opacity);
            egui::Frame::none()
                .fill(CARD)
                .rounding(16.0)
                .stroke(Stroke::new(1.0, BORDER))
                .shadow(ctx.style().visuals.window_shadow)
                .inner_margin(Margin::same(22.0))
                .show(ui, |ui| {
                    ui.set_width(width);
                    add(ui)
                })
                .inner
        })
        .inner
}

// ───────────────────────────── viewer / editor loading ─────────────────────────────

fn load_viewer(path: PathBuf) -> Modal {
    const LIMIT: u64 = 512 * 1024;
    let total = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut buf = Vec::new();
    if let Err(e) = fs::File::open(&path).and_then(|f| f.take(LIMIT).read_to_end(&mut buf)) {
        return Modal::Message { title: "Не удалось открыть файл".into(), text: e.to_string() };
    }
    let binary = buf.iter().take(8192).any(|&b| b == 0);
    let content = if binary {
        hex_dump(&buf[..buf.len().min(64 * 1024)])
    } else {
        String::from_utf8_lossy(&buf).into_owned()
    };
    let info = format!(
        "{} · {}{}",
        format_size(total),
        if binary { "двоичный, шестнадцатеричный вид" } else { "текст" },
        if total > LIMIT { " · показано начало" } else { "" }
    );
    Modal::Viewer { path, content, info }
}

fn load_editor(path: PathBuf) -> Modal {
    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if size > 4 * 1024 * 1024 {
        return Modal::Message { title: "Файл слишком большой".into(), text: "Встроенный редактор открывает файлы до 4 МБ.".into() };
    }
    match fs::read(&path) {
        Ok(bytes) => {
            if bytes.iter().take(8192).any(|&b| b == 0) {
                return Modal::Message { title: "Двоичный файл".into(), text: "Это не текстовый файл — откройте его на просмотр (F3).".into() };
            }
            match String::from_utf8(bytes) {
                Ok(text) => Modal::Editor { path, original: text.clone(), text, confirm_close: false },
                Err(_) => Modal::Message {
                    title: "Неподдерживаемая кодировка".into(),
                    text: "Редактировать можно только текст в UTF-8; для просмотра нажмите F3.".into(),
                },
            }
        }
        Err(e) => Modal::Message { title: "Не удалось открыть файл".into(), text: e.to_string() },
    }
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    #[test]
    fn native_clipboard_events_are_consumed_once() {
        let key = egui::Event::Key { key: Key::C, physical_key: Some(Key::C), pressed: true,
            repeat: false, modifiers: Modifiers::COMMAND };
        let mut events = vec![egui::Event::Copy, key, egui::Event::Cut,
            egui::Event::Paste(String::new()), egui::Event::Text("текст".into())];
        assert_eq!(clipboard_actions(&mut events), vec![Action::ClipboardCopy, Action::ClipboardCut, Action::ClipboardPaste]);
        assert!(matches!(events.as_slice(), [egui::Event::Text(_)]));
        assert!(clipboard_actions(&mut events).is_empty());
    }

    #[test]
    fn paste_targets_current_folder_and_keeps_sources_until_execution() {
        let root = std::env::temp_dir().join(format!("quokka-clipboard-{}", std::process::id()));
        fs::create_dir_all(root.join("source/папка/child")).unwrap();
        fs::create_dir_all(root.join("dest")).unwrap();
        let source = root.join("source/файл.txt");
        fs::write(&source, "данные").unwrap();
        let files = |cut| crate::clipboard::Files { paths: vec![source.clone()], cut };
        let copy = clipboard_operation(files(false), &root.join("dest")).unwrap();
        assert!(matches!(&copy, OpSpec::Copy { dest, overwrite: false, .. } if *dest == fs::canonicalize(root.join("dest")).unwrap()));
        let mv = clipboard_operation(files(true), &root.join("dest")).unwrap();
        assert!(matches!(mv, OpSpec::Move { overwrite: false, .. }));
        assert_eq!(fs::read_to_string(&source).unwrap(), "данные");
        // Copying into the same folder reaches fsops' unique-copy-name handling.
        assert!(clipboard_operation(files(false), &root.join("source")).is_ok());
        assert!(clipboard_operation(files(true), &root.join("source")).is_err());
        assert!(clipboard_operation(files(false), std::path::Path::new("")).is_err());
        assert!(clipboard_operation(files(false), &source).is_err());
        assert!(clipboard_operation(crate::clipboard::Files {
            paths: vec![root.join("source/папка")], cut: false,
        }, &root.join("source/папка/child")).is_err());
        fs::create_dir_all(root.join("moved")).unwrap();
        let mut queue = Queue::default();
        queue.push(copy);
        queue.push(clipboard_operation(crate::clipboard::Files {
            paths: vec![source.clone()], cut: true,
        }, &root.join("moved")).unwrap());
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut finished = Vec::new();
        while queue.is_active() {
            finished.extend(queue.poll(&ctx));
            assert!(std::time::Instant::now() < deadline, "clipboard operations timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(finished.len(), 2);
        assert!(finished.iter().all(|f| f.result.is_ok()));
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(root.join("dest/файл.txt")).unwrap(), "данные");
        assert_eq!(fs::read_to_string(root.join("moved/файл.txt")).unwrap(), "данные");
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod row_click_tests {
    use super::*;

    #[derive(Default)]
    struct Harness {
        ctx: egui::Context,
        time: f64,
    }

    impl Harness {
        fn frame(&mut self, events: Vec<egui::Event>, modifiers: Modifiers) -> Vec<usize> {
            self.time += 0.04;
            let mut opened = Vec::new();
            let _ = self.ctx.run(egui::RawInput {
                time: Some(self.time),
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))),
                events,
                modifiers,
                ..Default::default()
            }, |ctx| {
                begin_row_clicks(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    for row in 0..2 {
                        let rect = Rect::from_min_size(pos2(10.0, 10.0 + row as f32 * 40.0), vec2(200.0, 38.0));
                        let resp = ui.interact(rect, Id::new(("test-row", row)), Sense::click_and_drag());
                        if row_double_clicked(&resp, Id::new(("test-file", row))) {
                            opened.push(row);
                        }
                    }
                });
            });
            opened
        }

        fn click(&mut self, y: f32, button: egui::PointerButton, modifiers: Modifiers) -> Vec<usize> {
            let pos = pos2(30.0, y);
            assert!(self.frame(vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
                pos, button, pressed: true, modifiers,
            }], modifiers).is_empty());
            self.frame(vec![egui::Event::PointerButton { pos, button, pressed: false, modifiers }], modifiers)
        }

        fn primary(&mut self, y: f32) -> Vec<usize> {
            self.click(y, egui::PointerButton::Primary, Modifiers::NONE)
        }
    }

    #[test]
    fn switching_files_does_not_open_and_real_double_click_still_does() {
        let mut h = Harness::default();
        h.frame(vec![], Modifiers::NONE);
        assert!(h.primary(25.0).is_empty());
        assert!(h.primary(65.0).is_empty());
        assert!(h.primary(25.0).is_empty());
        // Let egui's triple-click sequence expire before a genuine double click.
        h.time += 1.0;
        assert!(h.primary(25.0).is_empty());
        assert_eq!(h.primary(25.0), vec![0]);
    }

    #[test]
    fn empty_space_context_menu_and_modified_clicks_break_the_sequence() {
        for interruption in 0..3 {
            let mut h = Harness::default();
            h.frame(vec![], Modifiers::NONE);
            assert!(h.primary(25.0).is_empty());
            let result = match interruption {
                0 => h.primary(150.0),
                1 => h.click(25.0, egui::PointerButton::Secondary, Modifiers::NONE),
                _ => h.click(25.0, egui::PointerButton::Primary, Modifiers::CTRL),
            };
            assert!(result.is_empty());
            assert!(h.primary(25.0).is_empty());
        }
    }

    #[test]
    fn slow_clicks_do_not_open() {
        let mut h = Harness::default();
        h.frame(vec![], Modifiers::NONE);
        assert!(h.primary(25.0).is_empty());
        h.time += 1.0;
        assert!(h.primary(25.0).is_empty());
    }
}

#[cfg(test)]
mod column_tests {
    use super::*;

    #[test]
    fn resize_preserves_small_movements_and_recovers_from_limits() {
        let drag = ColumnDrag { pointer_x: 500.0, width: 100.0, max_width: 200.0, visibility: (true, false) };
        assert!((drag.width_at(499.8) - 100.2).abs() < 0.001);
        assert_eq!(drag.width_at(900.0), config::MIN_COL_WIDTH);
        assert_eq!(drag.width_at(0.0), 200.0);
        assert_eq!(drag.width_at(500.0), 100.0);
    }

    #[test]
    fn hidden_columns_stay_hidden_while_resizing() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(580.0, 30.0));
        let widths = [170.0, 100.0, 150.0];
        let before = Cols::new(rect, &widths);
        assert!(before.kind.is_none());
        let after = Cols::with_visibility(rect, &[64.0, 100.0, 150.0], Some((true, false)));
        assert!(after.kind.is_none());
        assert_eq!(after.modified.unwrap().0, 416.0);
    }
}
