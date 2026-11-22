//! Main window: menu bar, icon sidebar, two tabbed panes, F1–F12 key bar, dialogs.

use quokka_domain::{tr, trf};
use crate::config::{self, Place, Settings};
use quokka_domain::lang::Lang;
use crate::fsops;
use crate::queue::{OpKind, OpSpec, Queue, State, Task};
use crate::icons::{self, Glyph};
use crate::iconsrc;
use crate::logo;
use crate::panel::{parent_of, root_label, Panel, SortKey};
use quokka_core::ports::Services;
use quokka_core::Waker;
use quokka_infra::fsops::StdFileOps;
use quokka_infra::xfer;
use quokka_domain::remote::{is_device, is_remote};
use crate::theme::{self, *};
use crate::util::*;
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, Id, Key, Layout, Margin, Modifiers, Rect, Response, RichText, Rounding,
    Sense, Stroke, Ui,
};
use std::fs;
use std::io::Read;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use quokka_infra::drives::{DriveInfo, DriveKind};
use std::sync::{Arc, Mutex};

mod drag;
mod find;
mod menubar;
mod rename;
mod preview;
mod columns;
mod remote;
mod updates;
mod convert;
mod hotkeys;
mod quickview;

const ROW_H: f32 = 28.0;
const TABS_H: f32 = 34.0;
const TOOL_H: f32 = 54.0;
const HEAD_H: f32 = 30.0;
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

/// egui counts double clicks globally, without checking the clicked row, and turns a
/// quick second double click (e.g. «..» twice in a row) into a triple click. So rows
/// pair their clicks themselves: row, time and spot of the last plain click.
#[derive(Clone, Default)]
struct RowClicks {
    previous: Option<(Id, f64, egui::Pos2)>,
    current: Option<(Id, f64, egui::Pos2)>,
}

/// How far (in points) the second click of a double click may land from the first:
/// a hand tremor, not a click on another part of the same wide row.
const DOUBLE_CLICK_DIST: f32 = 6.0;

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
    let (plain, now, releases, pointer) = resp.ctx.input(|i| {
        let releases: Vec<egui::Pos2> = i.events.iter().filter_map(|e| match e {
            egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: false, pos, .. } => Some(*pos),
            _ => None,
        }).collect();
        (!i.modifiers.any(), i.time, releases, i.pointer.interact_pos())
    });
    let Some(pos) = releases.last().copied().or(pointer) else { return false };
    let near = |a: egui::Pos2| a.distance(pos) <= DOUBLE_CLICK_DIST;
    let delay = resp.ctx.options(|o| o.input_options.max_double_click_delay);
    // A slow frame (e.g. a folder still loading after «..») can deliver both clicks
    // at once; the row then sees a single `clicked()`. Ask egui before `data_mut`:
    // `double_clicked()` locks the context too and would deadlock inside it.
    let both_in_frame = releases.len() >= 2 && near(releases[releases.len() - 2]) && resp.double_clicked();
    resp.ctx.data_mut(|d| {
        let clicks = d.get_temp_mut_or_default::<RowClicks>(Id::new("row-clicks"));
        let paired = clicks.previous.is_some_and(|(id, t, p)| id == target && now - t <= delay && near(p));
        let open = plain && (paired || both_in_frame);
        // Modified clicks select files and must also break the opening sequence;
        // after opening, the next click starts a fresh pair.
        clicks.current = (plain && !open).then_some((target, now, pos));
        open
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Help,
    About,
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
    CreateWindows,
    Quit,
    ToggleHidden,
    ToggleDual,
    /// The other pane shows the file under the cursor (Ctrl+Q).
    QuickView,
    Settings,
    ConfigureColumns(usize),
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
    SelectMask,
    UnselectMask,
    InvertSelection,
    OpenTerminal,
    ClearMarks,
    NewTab,
    CloseTab,
    EditPath,
    OpenInExplorer,
    Properties,
    FolderProperties,
    /// The Explorer context menu of the marked items (or the one under the cursor).
    WindowsMenu,
    /// The Explorer context menu of the empty area of the current folder.
    FolderWindowsMenu,
    /// The Explorer context menu / property sheet of a sidebar entry (drive, favorite).
    WindowsMenuOf(PathBuf),
    PropertiesOf(PathBuf),
    /// Safely remove a USB drive or eject a disc.
    Eject(PathBuf),
    OpenInNewTab(PathBuf),
    AddFavoritePath(PathBuf),
    AddCurrentFavorite,
    UndoDelete,
    FindFiles,
    MultiRename,
    CalculateSizes,
    CalculateAllSizes,
    CancelSizes,
    Connect,
    Disconnect,
    CheckUpdates,
    ConvertImages,
    /// Regular expressions in the name filter on / off (Alt+R).
    ToggleRegex,
}

fn fkeys() -> [(Key, &'static str, Action); 12] {
    [
        (Key::F1, tr!("Справка", "Help"), Action::Help),
        (Key::F2, tr!("Имя", "Name"), Action::Rename),
        (Key::F3, tr!("Просмотр", "View"), Action::View),
        (Key::F4, tr!("Правка", "Edit"), Action::Edit),
        (Key::F5, tr!("Копия", "Copy"), Action::Copy),
        (Key::F6, tr!("Перенос", "Move"), Action::Move),
        (Key::F7, tr!("Папка", "Folder"), Action::MkDir),
        (Key::F8, tr!("Удалить", "Delete"), Action::Delete),
        (Key::F9, tr!("Файл", "File"), Action::NewFile),
        (Key::F10, tr!("Выход", "Exit"), Action::Quit),
        (Key::F11, tr!("Скрытые", "Hidden"), Action::ToggleHidden),
        (Key::F12, tr!("Панели", "Panes"), Action::ToggleDual),
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
    Selection(bool),
    Zip(Vec<PathBuf>),
    Rename(PathBuf),
    MkDir,
    NewFile,
}

enum Modal {
    None,
    Help,
    About,
    Filter { first: bool },
    Settings { group: SettingsGroup, filter: String },
    PlaceEdit(Box<PlaceEditor>),
    Columns(columns::ColumnsDialog),
    Input { kind: InputKind, title: String, label: String, text: String, first: bool, error: Option<String> },
    Transfer { mv: bool, sources: Vec<PathBuf>, dest: String, overwrite: bool, first: bool },
    ConfirmDelete { sources: Vec<PathBuf>, permanent: bool },
    Preview(Box<preview::Preview>),
    Editor { path: PathBuf, text: String, original: String, confirm_close: bool },
    Message { title: String, text: String },
    Unzip { archives: Vec<PathBuf>, dest: String, subfolder: bool, overwrite: bool, first: bool },
    ConfirmQuit,
    Find(Box<find::FindDialog>),
    MultiRename(Box<rename::RenameDialog>),
    Connect(Box<remote::ConnectDialog>),
    Update(Box<updates::UpdateDialog>),
    Convert(Box<convert::ConvertDialog>),
}

/// What Ctrl+Z takes back.
enum Undo {
    /// Items the last recycle moved to the bin.
    Recycle(Vec<PathBuf>),
    /// (old, new) paths of the last group rename.
    Rename(Vec<(PathBuf, PathBuf)>),
    /// (from, to) paths of the items the last move put elsewhere.
    Move(Vec<(PathBuf, PathBuf)>),
}

struct Toast {
    text: String,
    error: bool,
    /// A button next to the text (e.g. «Отменить» that restores the last recycled
    /// items, «Открыть» for a new drive); such toasts stay longer.
    button: Option<(String, Action)>,
    born: Option<f64>,
}

impl Toast {
    fn new(text: impl Into<String>, error: bool) -> Toast {
        Toast { text: text.into(), error, button: None, born: None }
    }

    fn undo(text: impl Into<String>) -> Toast {
        Toast { button: Some((tr!("Отменить", "Undo").into(), Action::UndoDelete)), ..Toast::new(text, false) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsGroup {
    View,
    Search,
    Sidebar,
    Keys,
}

impl SettingsGroup {
    fn all() -> [(SettingsGroup, &'static str, Glyph); 4] {
        [
        (SettingsGroup::View, tr!("Вид и файлы", "View and files"), Glyph::Eye),
        (SettingsGroup::Search, tr!("Поиск", "Search"), Glyph::Search),
        (SettingsGroup::Sidebar, tr!("Избранное и меню", "Favorites and menu"), Glyph::Star),
        (SettingsGroup::Keys, tr!("Горячие клавиши", "Keyboard shortcuts"), Glyph::Keyboard),
        ]
    }

    /// Whether the page should stay in the tree for a (lower-cased) settings search.
    fn matches(self, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        let words = match self {
            SettingsGroup::Keys => return hotkeys::matches(q),
            SettingsGroup::View => tr!("колонки столбцы состав порядок расположение язык интерфейса english русский вид и файлы скрытые файлы иконки значки связанных приложений окно две панели внешний редактор команда f4 f11 f12 трей область уведомлений значок закрытии скрывать сворачивать несколько окон экземпляров запуск сеанс сессия вкладки восстанавливать", "columns visibility order arrangement interface language english russian view and files hidden files icons associated applications window two panes dual external editor command f4 f11 f12 tray notification area icon close hide minimize multiple windows instances startup session tabs restore"),
            SettingsGroup::Search => tr!("поиск фильтр по имени регулярные выражения regex регистр букв ctrl+f alt+r", "search filter by name regular expressions regex case sensitive letters ctrl+f alt+r"),
            SettingsGroup::Sidebar => tr!("избранное и меню боковое меню подписи диски закладки порядок восстановить", "favorites and menu sidebar labels drives bookmarks order restore"),
        };
        words.contains(q)
    }
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
fn icon_colors() -> [(Option<[u8; 3]>, &'static str); 12] {
    [
    (None, tr!("По умолчанию", "Default")),
    (Some([22, 119, 255]), tr!("Синий", "Blue")),
    (Some([38, 190, 160]), tr!("Бирюзовый", "Teal")),
    (Some([52, 186, 120]), tr!("Зелёный", "Green")),
    (Some([247, 182, 58]), tr!("Жёлтый", "Yellow")),
    (Some([255, 150, 70]), tr!("Оранжевый", "Orange")),
    (Some([240, 90, 90]), tr!("Красный", "Red")),
    (Some([236, 72, 153]), tr!("Розовый", "Pink")),
    (Some([139, 108, 240]), tr!("Фиолетовый", "Purple")),
    (Some([146, 98, 57]), tr!("Коричневый", "Brown")),
    (Some([100, 116, 139]), tr!("Серый", "Gray")),
    (Some([33, 39, 55]), tr!("Чёрный", "Black")),
]
}

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
    /// The last action Ctrl+Z / «Отменить» can take back.
    undo: Option<Undo>,
    /// Downloaded copies of server files to open once their copy job finishes.
    open_after: Vec<PathBuf>,
    drives: Vec<PathBuf>,
    /// Kind, label and (total, free) bytes per drive, read in the background for the
    /// sidebar (a disconnected network drive can take seconds to answer).
    drive_info: Arc<Mutex<HashMap<PathBuf, DriveInfo>>>,
    /// Drives being read right now: at most one read per drive at a time.
    drive_busy: Arc<Mutex<HashSet<PathBuf>>>,
    /// When each drive was last read (the hovered one is re-read every second).
    drive_read_at: HashMap<PathBuf, std::time::Instant>,
    /// Notices drives being plugged in or removed.
    drive_watch: Option<quokka_infra::drives::DriveWatch>,
    /// The infrastructure adapters every tab works with.
    services: Services,
    actions: Vec<Action>,
    search_id: Id,
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
    /// Ctrl+Q: the pane that shows the file under the cursor instead of a folder.
    quick_view: Option<quickview::QuickView>,
    /// Drop targets of both panes, as drawn last.
    zones: [Option<drag::DropZone>; 2],
    /// Native window handle, for dragging files out to other apps.
    hwnd: isize,
    /// A drag to another app has ended: its mouse-up never reached egui, send one.
    release_pointer: bool,
    /// Set when a Ctrl+V press has pasted; further paste signals of the same
    /// physical press are dropped until V is released (see [`dedupe_paste`]).
    paste_latch: Option<std::time::Instant>,
    /// The window is hooked for the tray icon and the single-instance hand-over.
    tray_ready: bool,
    /// The user asked to quit (F10, tray «Выход»): closing must not just hide to the tray.
    exit_requested: bool,
    /// Results of the update check and download threads.
    update_inbox: updates::Inbox,
    /// A newer release found by the start-up check, shown when no dialog is open.
    update_offer: Option<quokka_infra::update::Release>,
}

/// The UI's implementation of the core "redraw" signal.
pub(crate) fn waker(ctx: &egui::Context) -> Waker {
    let ctx = ctx.clone();
    Waker::new(move || ctx.request_repaint())
}

fn path_edit_id(idx: usize) -> Id {
    Id::new(("path-edit", idx))
}

fn clipboard_actions(events: &mut Vec<egui::Event>, modifiers: Modifiers) -> Vec<Action> {
    let mut actions = Vec::new();
    let shift_del = hotkeys::with(|m| m.action_for(&hotkeys::Combo::new(false, false, true, Key::Delete)));
    events.retain(|event| {
        let action = match event {
            egui::Event::Copy => Some(Action::ClipboardCopy),
            // On Windows egui-winit turns Shift+Del (a legacy "cut" shortcut) into
            // `Event::Cut` and drops the key event, so the Shift+Delete binding never fires.
            // In the file list Shift+Del means "delete permanently", like in Explorer.
            // It does whatever Shift+Del is assigned to (by default "delete permanently").
            egui::Event::Cut if modifiers.shift && !modifiers.command => shift_del.clone(),
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

/// Turn every signal of one physical Ctrl+V press into exactly one paste.
///
/// A press reaches us several ways: egui's `Paste` (clipboard has text), a
/// `Key::V` press (e.g. Cyrillic layout), and the native key state polled with
/// `GetAsyncKeyState` (files-only clipboard, where egui-winit swallows Ctrl+V).
/// The polled state runs ahead of the window messages, so these signals land in
/// different frames and used to paste the files twice. After the first paste
/// the latch drops the rest until V is up and its key-up event has arrived
/// (or, if that event never comes, a short timeout has passed).
fn dedupe_paste(
    events: &mut Vec<egui::Event>,
    down: bool,
    typing: bool,
    allow: bool,
    latch: &mut Option<std::time::Instant>,
    now: std::time::Instant,
) {
    let is_press = |e: &egui::Event| match e {
        egui::Event::Paste(_) => true,
        egui::Event::Key { key: Key::V, pressed: true, modifiers, .. } => modifiers.command,
        _ => false,
    };
    let v_up = events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::V, pressed: false, .. }));
    if !typing {
        let released = events.iter().any(|e| matches!(e,
            egui::Event::Key { key: Key::V, pressed: false, modifiers, .. } if modifiers.command));
        let native = events.iter().any(is_press);
        if latch.is_some() {
            events.retain(|e| !is_press(e));
        } else if down || released || native {
            if allow && !native {
                events.push(egui::Event::Paste(String::new()));
            }
            if allow || native {
                *latch = Some(now);
            }
        }
    }
    if let Some(at) = *latch {
        let settled = v_up || now.duration_since(at) > std::time::Duration::from_millis(300);
        if !down && settled && !events.iter().any(is_press) {
            *latch = None;
        }
    }
}

fn clipboard_operation(files: crate::clipboard::Files, dest: &std::path::Path) -> Result<OpSpec, String> {
    if dest.as_os_str().is_empty() || !dest.is_dir() {
        return Err(tr!("Откройте папку для вставки файлов", "Open a folder to paste files").into());
    }
    let dest = fs::canonicalize(dest).map_err(|e| trf!("Не удалось открыть папку: {e}", "Failed to open the folder: {e}"))?;
    let mut sources = Vec::new();
    for path in files.paths {
        let source = fs::canonicalize(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if source.is_dir() && dest.starts_with(&source) {
            return Err(tr!("Нельзя вставить папку в саму себя или в её подпапку", "Cannot paste a folder into itself or its subfolder").into());
        }
        if files.cut && source.parent() == Some(dest.as_path()) { continue; }
        if !sources.contains(&source) { sources.push(source); }
    }
    if sources.is_empty() { return Err(tr!("Нет файлов для вставки в эту папку", "No files to paste into this folder").into()); }
    Ok(if files.cut {
        OpSpec::Move { sources, dest, overwrite: false }
    } else {
        OpSpec::Copy { sources, dest, overwrite: false }
    })
}

impl QuokkaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<PathBuf>, updated: bool) -> Self {
        theme::apply(&cc.egui_ctx);
        let home = config::home();
        let settings = Settings::load();
        hotkeys::install(hotkeys::Keymap::with_overrides(&settings.hotkeys));
        let services = quokka_infra::services(waker(&cc.egui_ctx));
        let drives = services.fs.drives();
        let right_start = drives.first().cloned().unwrap_or_else(|| home.clone());
        let mk = |path: PathBuf| {
            let mut p = Panel::new(path, settings.show_hidden, services.clone());
            p.filter_regex = settings.regex;
            p.filter_case = settings.case_sensitive;
            p
        };
        let mut sides = [Side::new(mk(home.clone())), Side::new(mk(right_start))];
        let mut active = 0;
        if settings.restore_session {
            let sess = &settings.session;
            for (pane, side) in sides.iter_mut().enumerate() {
                let tabs: Vec<Panel> = sess.tabs[pane].iter().map(|p| mk(restorable(p))).collect();
                if !tabs.is_empty() {
                    side.cur = sess.cur[pane].min(tabs.len() - 1);
                    side.tabs = tabs;
                }
            }
            active = sess.active.min(1);
        }
        // a folder given on the command line opens in a new tab of the left pane
        if let Some(start) = start {
            let side = &mut sides[0];
            side.tabs.push(mk(start));
            side.cur = side.tabs.len() - 1;
            active = 0;
        }
        let mut app = QuokkaApp {
            sides,
            active,
            pane_rects: [None, None],
            settings,
            modal: Modal::None,
            queue: Queue::default(),
            queue_open: false,
            quitting: false,
            allow_close: false,
            toast: None,
            undo: None,
            open_after: Vec::new(),
            drive_watch: Some(quokka_infra::drives::DriveWatch::start(std::time::Duration::from_millis(700), {
                let waker = services.waker.clone();
                move || waker.wake()
            })),
            drives,
            drive_info: Default::default(),
            drive_busy: Default::default(),
            drive_read_at: HashMap::new(),
            services,
            actions: Vec::new(),
            search_id: Id::new("search-box"),
            logo: logo::texture(&cc.egui_ctx, 128),
            ctx: cc.egui_ctx.clone(),
            side_drag: None,
            icon_cache: IconCache::new(),
            file_icons: crate::file_icons::FileIcons::new(&cc.egui_ctx),
            find: None,
            resume_find: false,
            file_drag: None,
            quick_view: None,
            zones: [None, None],
            hwnd: 0,
            release_pointer: false,
            paste_latch: None,
            tray_ready: false,
            exit_requested: false,
            update_inbox: Default::default(),
            update_offer: None,
        };
        app.refresh_drive_space();
        if updated {
            app.toast(trf!("Quokka обновлена до версии {}", "Quokka has been updated to version {}", env!("CARGO_PKG_VERSION")), false);
        }
        app.auto_check_updates();
        app
    }

    /// Re-read every drive's label and space in the background.
    fn refresh_drive_space(&mut self) {
        for d in self.drives.clone() {
            self.refresh_drive(&d);
        }
    }

    /// Re-read one drive's label and space on a background thread, unless a read of
    /// it is still running (a dead network drive must not pile up threads).
    fn refresh_drive(&mut self, d: &Path) {
        if !self.drive_busy.lock().unwrap_or_else(|e| e.into_inner()).insert(d.to_path_buf()) {
            return;
        }
        self.drive_read_at.insert(d.to_path_buf(), std::time::Instant::now());
        let (d, out, busy, waker) = (d.to_path_buf(), self.drive_info.clone(), self.drive_busy.clone(), self.services.waker.clone());
        std::thread::spawn(move || {
            let info = quokka_infra::drives::info(&d);
            let changed = {
                let mut map = out.lock().unwrap_or_else(|e| e.into_inner());
                map.insert(d.clone(), info.clone()) != Some(info)
            };
            busy.lock().unwrap_or_else(|e| e.into_inner()).remove(&d);
            if changed {
                waker.wake();
            }
        });
    }

    fn drive(&self, d: &Path) -> Option<DriveInfo> {
        self.drive_info.lock().unwrap_or_else(|e| e.into_inner()).get(d).cloned()
    }

    /// Sidebar / notification name of a drive: «KINGSTON (E:)», «Локальный диск (C:)».
    fn drive_title(&self, d: &Path) -> String {
        let letter = root_label(d);
        if is_device(d) {
            // a phone or camera: Windows' name for it is the whole title
            return letter;
        }
        match self.drive(d) {
            Some(i) if !i.label.is_empty() => format!("{} ({letter})", i.label),
            Some(i) => format!("{} ({letter})", drive_kind_name(i.kind)),
            None => format!("{} ({letter})", drive_kind_name(quokka_infra::drives::kind(d))),
        }
    }

    /// Pick up drives plugged in or removed since the last frame.
    fn poll_drives(&mut self) {
        let Some(change) = self.drive_watch.as_ref().and_then(|w| w.poll()) else { return };
        self.drives = self.services.fs.drives();
        {
            let mut map = self.drive_info.lock().unwrap_or_else(|e| e.into_inner());
            for d in &change.removed {
                map.remove(d);
            }
        }
        for d in &change.added {
            self.refresh_drive(d);
        }
        // tabs on a drive that is gone fall back to «This PC»
        for si in 0..2 {
            for p in &mut self.sides[si].tabs {
                if !p.is_pc() && change.removed.iter().any(|r| on_drive(&p.path, r)) {
                    let _ = p.navigate(PathBuf::new());
                }
            }
        }
        self.reload_dirs(&[]);
        if change.initial {
            // devices that were already there when Quokka started
        } else if let Some(d) = change.added.first() {
            // the label is not known yet: name the device by its kind and letter
            let text = if is_device(d) {
                trf!("Подключено устройство: {}", "Device connected: {}", root_label(d))
            } else {
                let kind = drive_kind_name(quokka_infra::drives::kind(d));
                trf!("Подключено устройство: {} ({})", "Device connected: {} ({})", kind, root_label(d))
            };
            self.toast = Some(Toast { button: Some((tr!("Открыть", "Open").into(), Action::GoTo(d.clone()))), ..Toast::new(text, false) });
        } else if let Some(d) = change.removed.first() {
            self.toast(trf!("Устройство {} отключено", "Device {} removed", root_label(d)), false);
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

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some(Toast::new(text, error));
    }

    fn report(&mut self, r: Result<(), String>) {
        if let Err(e) = r {
            self.toast(e, true);
        }
    }

    /// Whether tab `tab` of side `side` is on screen now.
    fn shown(&self, side: usize, tab: usize) -> bool {
        self.sides[side].cur == tab && (self.settings.dual || side == self.active)
    }

    /// Refresh the tabs on screen; the others refresh when they are next shown.
    fn reload_all(&mut self) {
        for si in 0..2 {
            for ti in 0..self.sides[si].tabs.len() {
                let shown = self.shown(si, ti);
                let p = &mut self.sides[si].tabs[ti];
                if shown {
                    p.reload();
                } else {
                    p.stale = true;
                }
            }
        }
    }

    /// Refresh only the tabs showing one of `dirs` (plus search results and
    /// «This PC», whose rows can come from anywhere).
    fn reload_dirs(&mut self, dirs: &[PathBuf]) {
        let key = |p: &std::path::Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
        let dirs: Vec<String> = dirs.iter().map(|d| key(d)).collect();
        for si in 0..2 {
            for ti in 0..self.sides[si].tabs.len() {
                let shown = self.shown(si, ti);
                let p = &mut self.sides[si].tabs[ti];
                if !(p.results.is_some() || p.is_pc() || dirs.contains(&key(&p.path))) {
                    continue;
                }
                if shown {
                    p.reload();
                } else {
                    p.stale = true;
                }
            }
        }
    }

    /// Open tabs of both panes, as saved for the next start.
    fn session(&self) -> config::Session {
        config::Session {
            tabs: [0, 1].map(|i| self.sides[i].tabs.iter().map(|p| p.path.clone()).collect()),
            cur: [self.sides[0].cur, self.sides[1].cur],
            active: self.active,
        }
    }

    /// Save the open tabs when they changed (cheap to compare every frame).
    fn save_session(&mut self) {
        let sess = self.session();
        if sess != self.settings.session {
            self.settings.session = sess;
            if self.settings.restore_session {
                self.save_settings();
            }
        }
    }

    /// Show `path` in the active pane: its tab if one shows it, otherwise a new tab.
    fn open_in_new_tab(&mut self, path: PathBuf) {
        let side = &mut self.sides[self.active];
        if let Some(i) = side.tabs.iter().position(|p| p.path == path && p.results.is_none()) {
            side.cur = i;
            return;
        }
        let mut p = Panel::new(path, self.settings.show_hidden, self.services.clone());
        p.filter_regex = self.settings.regex;
        p.filter_case = self.settings.case_sensitive;
        side.tabs.insert(side.cur + 1, p);
        side.cur += 1;
    }

    /// Tray icon, close-to-tray and folders handed over by another launch.
    fn tray_frame(&mut self, ctx: &egui::Context) {
        if !self.tray_ready && self.hwnd != 0 {
            crate::tray::install(self.hwnd, ctx);
            self.tray_ready = true;
        }
        crate::tray::set_icon(self.settings.tray_icon || self.settings.close_to_tray);
        let events = crate::tray::take_events();
        for path in events.open {
            self.open_in_new_tab(path);
        }
        if events.quit {
            self.exit_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            self.toast(trf!("Не удалось сохранить настройки: {e}", "Failed to save settings: {e}"), true);
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
                    Place::new(tr!("Этот компьютер", "This PC"), PathBuf::new(), Glyph::Desktop)
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
            .is_some_and(|f| f == path_edit_id(0) || f == path_edit_id(1))
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
            // the user's shortcuts first, matched exactly: egui's `consume_key` matches
            // modifiers loosely (`NONE, ArrowUp` would also eat Alt+↑)
            acts.extend(hotkeys::take_actions(&mut i.events, typing));
            if i.consume_key(Modifiers::NONE, Key::Tab) {
                acts.push(Action::SwitchPanel);
            }
            let nav = [
                (Key::ArrowUp, Action::Cursor(-1)),
                (Key::ArrowDown, Action::Cursor(1)),
                (Key::PageUp, Action::Cursor(-page)),
                (Key::PageDown, Action::Cursor(page)),
            ];
            // every auto-repeat of the frame counts: `consume_key` takes one press
            for (k, a) in nav {
                while i.consume_key(Modifiers::NONE, k) {
                    acts.push(a.clone());
                }
            }
            if !typing {
                let mods = i.modifiers;
                acts.extend(clipboard_actions(&mut i.events, mods));
                let keys = [
                    (Modifiers::NONE, Key::Home, Action::CursorFirst),
                    (Modifiers::NONE, Key::End, Action::CursorLast),
                    (Modifiers::NONE, Key::Enter, Action::Open),
                    (Modifiers::NONE, Key::Backspace, Action::GoUp),
                    (Modifiers::NONE, Key::Insert, Action::MarkMove),
                    (Modifiers::NONE, Key::Space, Action::Mark),
                    (Modifiers::NONE, Key::Escape, Action::ClearMarks),
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
                let ok = (f == path_edit_id(0) && editing(0))
                    || (f == path_edit_id(1) && editing(1));
                if !ok {
                    ctx.memory_mut(|m| m.surrender_focus(f));
                }
            }
        }
    }

    // ───────────────────────────── actions ─────────────────────────────

    fn run(&mut self, a: Action, ctx: &egui::Context) {
        if is_remote(&self.ap_ref().path) && remote::local_only(&a) {
            self.toast(tr!("На сервере это действие недоступно", "This action is not available on a server"), true);
            return;
        }
        // a server folder we are not logged in to (favorite, typed path, history): log in first
        if let Action::GoTo(p) = &a {
            if let Some((site, _)) = quokka_domain::remote::split(p).filter(|_| is_device(p)) {
                if !quokka_infra::net::is_connected(&site) {
                    self.toast(trf!("Устройство «{}» не подключено", "Device “{}” is not connected", site.host), true);
                    return;
                }
            }
            if quokka_domain::remote::split(p).is_some_and(|(site, _)| !quokka_infra::net::is_connected(&site)) {
                let p = p.clone();
                self.open_connect(Some(&p));
                return;
            }
        }
        if quokka_infra::archive::location(&self.ap_ref().path).is_some() && matches!(a,
            Action::Rename | Action::MultiRename | Action::Move | Action::Delete | Action::DeletePermanent |
            Action::Edit | Action::EditBuiltin | Action::MkDir | Action::NewFile | Action::CreateWindows |
            Action::ClipboardCut | Action::ClipboardCopy | Action::ClipboardPaste | Action::Zip | Action::Unzip | Action::FindFiles | Action::ConvertImages |
            Action::WindowsMenu | Action::FolderWindowsMenu) {
            self.toast(tr!("Архив открыт только для чтения. F5 — извлечь выбранное.", "Archive is read-only. F5 extracts selected items."), true);
            return;
        }
        match a {
            Action::OpenTerminal => {
                let path = self.ap_ref().path.clone();
                if let Some((site, dir)) = quokka_domain::remote::split(&path) {
                    self.open_ssh_terminal(&site, Some(&dir));
                } else if let Err(e) = fsops::open_terminal(&path) {
                    self.toast(e.to_string(), true);
                }
            }
            Action::InvertSelection => self.ap().invert_marks(),
            Action::SelectMask | Action::UnselectMask => {
                let select = a == Action::SelectMask;
                self.modal = Modal::Input {
                    kind: InputKind::Selection(select),
                    title: if select { tr!("Выделить по маске", "Select by mask") } else { tr!("Снять выделение по маске", "Deselect by mask") }.into(),
                    label: tr!("* — любые символы, ? — один символ; несколько масок через ;", "* — any characters, ? — one character; separate masks with ;").into(),
                    text: "*.*".into(), first: true, error: None,
                };
            }
            Action::CalculateSizes | Action::CalculateAllSizes => {
                let count = self.ap().calculate_sizes(a == Action::CalculateAllSizes);
                if count == 0 { self.toast(tr!("Выберите папку для расчёта размера", "Select a folder to calculate its size"), false); }
            }
            Action::CancelSizes => self.ap().folder_sizes.stop(),
            Action::Help => self.modal = Modal::Help,
            Action::Connect => self.open_connect(None),
            Action::Disconnect => self.disconnect_current(),
            Action::About => self.modal = Modal::About,
            Action::FindFiles => self.open_find(),
            Action::UndoDelete => match self.undo.take() {
                Some(Undo::Recycle(originals)) => {
                    self.toast = None;
                    self.enqueue(OpSpec::Restore { originals });
                }
                Some(Undo::Move(pairs)) => {
                    self.toast = None;
                    let pairs = pairs.into_iter().rev().map(|(from, to)| (to, from)).collect();
                    self.enqueue(OpSpec::MoveBack { pairs });
                }
                Some(Undo::Rename(pairs)) => {
                    self.toast = None;
                    let back: Vec<(PathBuf, PathBuf)> = pairs.into_iter().map(|(old, new)| (new, old)).collect();
                    let dirs: Vec<PathBuf> = back.iter().filter_map(|(_, p)| p.parent().map(PathBuf::from)).collect();
                    match quokka_infra::rename::rename_batch(&back) {
                        Ok(done) => self.toast(trf!("Переименование отменено: {}", "Rename undone: {}", items_text(done.len() as u64)), false),
                        Err(e) => self.toast(e, true),
                    }
                    self.reload_dirs(&dirs);
                }
                None => self.toast(tr!("Нечего отменять", "Nothing to undo"), false),
            },
            Action::MultiRename => self.open_multi_rename(),
            Action::ConvertImages => self.open_convert(ctx),
            Action::ConfigureColumns(pane) => {
                self.modal = Modal::Columns(columns::ColumnsDialog::new(pane, self.settings.columns[pane].clone(), false));
            }
            Action::Settings => self.modal = Modal::Settings { group: SettingsGroup::View, filter: String::new() },
            // with quick view on, the other pane is the preview: there is nothing to switch to
            Action::SwitchPanel if self.quick_view.is_some() => {}
            Action::SwitchPanel => self.active = 1 - self.active,
            Action::QuickView => {
                self.quick_view = match self.quick_view {
                    Some(_) => None,
                    None => Some(quickview::QuickView::new(ctx, 1 - self.active)),
                };
            }
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
                self.drives = self.services.fs.drives();
                self.refresh_drive_space();
                self.reload_all();
            }
            Action::ToggleHidden => {
                self.settings.show_hidden = !self.settings.show_hidden;
                self.sync_settings();
                self.save_settings();
                let h = self.settings.show_hidden;
                self.toast(if h { tr!("Скрытые файлы показаны", "Hidden files are shown") } else { tr!("Скрытые файлы не показываются", "Hidden files are not shown") }, false);
            }
            Action::ToggleDual => {
                self.settings.dual = !self.settings.dual;
                self.save_settings();
            }
            Action::Quit => {
                self.exit_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Action::FocusSearch => {
                self.modal = Modal::Filter { first: true };
            }
            Action::ClearSearch => {
                let p = self.ap();
                p.filter.clear();
                p.apply_filter();
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
                    self.toast(trf!("Не удалось открыть Проводник: {e}", "Failed to open Explorer: {e}"), true);
                }
            }
            Action::CheckUpdates => self.check_updates_now(),
            Action::ToggleRegex => {
                self.settings.regex = !self.settings.regex;
                self.sync_settings();
                self.save_settings();
            }
            Action::Properties | Action::FolderProperties => {
                let p = self.ap_ref();
                let mut paths = if matches!(a, Action::Properties) { p.targets() } else { Vec::new() };
                if paths.is_empty() {
                    match p.current() {
                        Some(e) if e.is_drive && matches!(a, Action::Properties) => paths.push(e.path.clone()),
                        _ if !p.is_pc() => paths.push(p.path.clone()),
                        _ => {}
                    }
                }
                if paths.is_empty() {
                    return;
                }
                if paths.iter().any(|path| quokka_infra::archive::is_member(path)) {
                    self.toast(tr!("Свойства недоступны внутри архива", "Properties are not available inside an archive"), true);
                    return;
                }
                if let Err(e) = quokka_infra::shell_props::show(&paths) {
                    self.toast(trf!("Не удалось открыть свойства: {e}", "Could not open properties: {e}"), true);
                }
            }
            Action::WindowsMenuOf(path) => {
                let path = shell_name(&path);
                #[cfg(windows)]
                match quokka_infra::shell_menu::show(self.hwnd, &[path.clone()], &path) {
                    Ok(quokka_infra::shell_menu::Picked::Done) => self.refresh_drive_space(),
                    Ok(_) => {}
                    Err(e) => self.toast(trf!("Не удалось показать меню Windows: {e}", "Could not show the Windows menu: {e}"), true),
                }
                #[cfg(not(windows))]
                let _ = path;
            }
            Action::PropertiesOf(path) => {
                if let Err(e) = quokka_infra::shell_props::show(&[shell_name(&path)]) {
                    self.toast(trf!("Не удалось открыть свойства: {e}", "Could not open properties: {e}"), true);
                }
            }
            Action::Eject(path) => {
                // leave the drive in every tab so Windows can lock the volume
                for si in 0..2 {
                    for p in &mut self.sides[si].tabs {
                        if p.path.starts_with(&path) {
                            let _ = p.navigate(PathBuf::new());
                        }
                    }
                }
                #[cfg(windows)]
                if let Err(e) = quokka_infra::shell_menu::invoke_verb(self.hwnd, &path, "eject") {
                    self.toast(trf!("Не удалось извлечь {}: {e}", "Could not eject {}: {e}", root_label(&path)), true);
                }
            }
            Action::OpenInNewTab(path) => self.open_in_new_tab(path),
            Action::AddFavoritePath(path) => self.place_op(PlaceOp::AddPath(path), false),
            Action::WindowsMenu | Action::FolderWindowsMenu => {
                #[cfg(windows)]
                {
                    let p = self.ap_ref();
                    let mut items = Vec::new();
                    if a == Action::WindowsMenu {
                        items = p.targets();
                        if items.is_empty() {
                            if let Some(e) = p.current().filter(|e| e.is_drive) {
                                items.push(e.path.clone());
                            }
                        }
                    }
                    if items.is_empty() && p.is_pc() {
                        return;
                    }
                    let folder = p.path.clone();
                    match quokka_infra::shell_menu::show(self.hwnd, &items, &folder) {
                        Ok(quokka_infra::shell_menu::Picked::Rename) => self.run(Action::Rename, ctx),
                        Ok(quokka_infra::shell_menu::Picked::Done) => {
                            if !folder.as_os_str().is_empty() {
                                self.reload_dirs(&[folder]);
                            }
                        }
                        Ok(quokka_infra::shell_menu::Picked::None) => {}
                        Err(e) => self.toast(trf!("Не удалось показать меню Windows: {e}", "Could not show the Windows menu: {e}"), true),
                    }
                }
            }
            Action::EditPath => {
                let p = self.ap();
                p.editing_path = Some(remote::shown(&p.path));
                p.focus_path_edit = true;
            }
            Action::Rename if self.ap_ref().marked.len() > 1 => self.open_multi_rename(),
            Action::Rename => {
                let Some(e) = self.ap_ref().current().cloned() else { return };
                if e.is_parent || e.is_drive {
                    return;
                }
                self.modal = Modal::Input {
                    kind: InputKind::Rename(e.path.clone()),
                    title: tr!("Переименование", "Rename").into(),
                    label: trf!("Новое имя для «{}»", "New name for “{}”", e.name),
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
                    self.toast(tr!("Ничего не выбрано", "Nothing selected"), true);
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
                let base = if base.is_empty() { tr!("Архив", "Archive").to_string() } else { base };
                let name = format!("{base}.zip");
                // like F5: the archive goes to the other panel
                let dir = self.dest_dir();
                let text = if dir.as_os_str().is_empty() { name } else { dir.join(name).display().to_string() };
                let label = trf!("Будет упаковано: {}", "Will be packed: {}", items_text(sources.len() as u64));
                self.modal = Modal::Input {
                    kind: InputKind::Zip(sources),
                    title: tr!("Создать ZIP-архив", "Create ZIP archive").into(),
                    label,
                    text,
                    first: true,
                    error: None,
                };
            }
            Action::Unzip => {
                let archives: Vec<PathBuf> = self.ap_ref().targets().into_iter().filter(|p| is_zip(p)).collect();
                if archives.is_empty() {
                    self.toast(tr!("Выберите ZIP-архив", "Select a ZIP archive"), true);
                    return;
                }
                // like F5: unpack into the other panel
                let dest = self.dest_dir().display().to_string();
                self.modal = Modal::Unzip { archives, dest, subfolder: true, overwrite: false, first: true };
            }
            Action::ToggleQueue => self.queue_open = !self.queue_open,
            Action::CreateWindows => {
                #[cfg(windows)]
                if !self.ap_ref().is_pc() {
                    let path = self.ap_ref().path.clone();
                    match quokka_infra::shell_new::show(self.hwnd, &path) {
                        Ok(Some(created)) => {
                            let name = created.file_name().unwrap_or_default().to_string_lossy().into_owned();
                            self.modal = Modal::Input {
                                kind: InputKind::Rename(created),
                                title: tr!("Имя нового файла", "New file name").into(),
                                label: trf!("Создать в {}", "Create in {}", path.display()),
                                text: name,
                                first: true,
                                error: None,
                            };
                        }
                        Ok(None) => {}
                        Err(error) => self.toast(trf!("Не удалось создать файл: {}", "Could not create file: {}", error), true),
                    }
                    self.reload_dirs(&[path]);
                }
            }
            Action::MkDir | Action::NewFile => {
                if self.ap_ref().is_pc() {
                    self.toast(tr!("Сначала откройте папку", "Open a folder first"), true);
                    return;
                }
                let dir = a == Action::MkDir;
                self.modal = Modal::Input {
                    kind: if dir { InputKind::MkDir } else { InputKind::NewFile },
                    title: if dir { tr!("Новая папка", "New folder") } else { tr!("Новый файл", "New file") }.into(),
                    label: trf!("Создать в {}", "Create in {}", remote::shown(&self.ap_ref().path)),
                    text: if dir { tr!("Новая папка", "New folder") } else { tr!("Новый файл.txt", "New file.txt") }.into(),
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
                    self.toast(tr!("Папку нельзя редактировать", "A folder cannot be edited"), true);
                } else if a == Action::EditBuiltin {
                    self.modal = load_editor(e.path);
                } else if let Err(err) = fsops::open_in_editor(&self.settings.editor, &e.path) {
                    self.toast(trf!("Не удалось запустить редактор: {err}. Встроенный — Shift+F4", "Failed to start the editor: {err}. Built-in one — Shift+F4"), true);
                }
            }
            Action::Copy | Action::Move => {
                let sources = self.ap_ref().targets();
                if sources.is_empty() {
                    self.toast(tr!("Ничего не выбрано", "Nothing selected"), true);
                    return;
                }
                let mut other = self.other_path();
                if quokka_infra::archive::location(&other).is_some() { other = PathBuf::new(); }
                let dest = if other.as_os_str().is_empty() { String::new() } else { remote::shown(&other) };
                self.modal = Modal::Transfer { mv: a == Action::Move, sources, dest, overwrite: false, first: true };
            }
            Action::ClipboardCopy | Action::ClipboardCut => {
                let paths = self.ap_ref().targets();
                if paths.is_empty() {
                    self.toast(tr!("Ничего не выбрано", "Nothing selected"), true);
                    return;
                }
                let cut = a == Action::ClipboardCut;
                let count = paths.len();
                match crate::clipboard::write(self.hwnd, &crate::clipboard::Files { paths, cut }) {
                    Ok(()) => self.toast(trf!("{}: {}. Перейдите в папку и нажмите Ctrl+V", "{}: {}. Go to a folder and press Ctrl+V",
                        if cut { tr!("Вырезано", "Cut") } else { tr!("В буфере обмена", "Copied to clipboard") }, items_text(count as u64)), false),
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
                    self.toast(tr!("Ничего не выбрано", "Nothing selected"), true);
                    return;
                }
                // a server has no Recycle Bin
                let permanent = a == Action::DeletePermanent || !fsops::HAS_RECYCLE_BIN || sources.iter().any(|s| is_remote(s));
                self.modal = Modal::ConfirmDelete { sources, permanent };
            }
        }
    }

    fn open_current(&mut self) {
        let Some(e) = self.ap_ref().current().cloned() else { return };
        if e.is_dir || (is_zip(&e.path) && !quokka_infra::archive::is_member(&e.path)) {
            let r = self.ap().navigate(e.path);
            self.report(r);
        } else if quokka_infra::archive::is_member(&e.path) {
            self.modal = load_viewer(e.path);
        } else if is_remote(&e.path) {
            let dir = remote::temp_copy_dir(&e.path);
            if let Err(err) = fs::create_dir_all(&dir) {
                self.toast(err.to_string(), true);
                return;
            }
            self.open_after.push(dir.join(&e.name));
            self.enqueue(OpSpec::Copy { sources: vec![e.path], dest: dir, overwrite: true });
        } else if let Err(err) = fsops::open_external(&e.path) {
            self.toast(trf!("Не удалось открыть «{}»: {}", "Failed to open “{}”: {}", e.name, err), true);
        } else {
            let feedback = OpenFeedback { path: e.path, started: self.ctx.input(|i| i.time) };
            self.ctx.data_mut(|d| d.insert_temp(open_feedback_id(), feedback));
            self.ctx.request_repaint();
        }
    }

    fn apply_input(&mut self, kind: &InputKind, text: &str) -> Result<(), String> {
        let name = text.trim();
        if name.is_empty() {
            return Err(tr!("Имя не может быть пустым", "Name cannot be empty").into());
        }
        let dir = self.ap_ref().path.clone();
        let select;
        match kind {
            InputKind::Selection(select) => return self.ap().mark_mask(name, *select),
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
                    return Err(trf!("«{}» уже существует", "“{}” already exists", target.display()));
                }
                if target.parent().is_some_and(|p| !p.is_dir()) {
                    return Err(tr!("Папка назначения не существует", "Destination folder does not exist").into());
                }
                self.enqueue(OpSpec::Zip { sources: sources.clone(), dest: target });
                return Ok(());
            }
            InputKind::Rename(old) => {
                if name.contains(['/', '\\']) {
                    return Err(tr!("Имя не может содержать косую черту", "Name cannot contain a slash").into());
                }
                if is_remote(old) {
                    let new = quokka_domain::remote::parent(old).and_then(|d| quokka_domain::remote::join(&d, name)).unwrap_or_default();
                    // names on a server are case-sensitive
                    if xfer::exists(&new) {
                        return Err(trf!("«{name}» уже существует", "“{name}” already exists"));
                    }
                    xfer::rename(old, &new).map_err(|e| e.to_string())?;
                    self.reload_all();
                    self.ap().select_name(name);
                    return Ok(());
                }
                let new = old.with_file_name(name);
                let old_name = old.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                if new.exists() && old_name != name.to_lowercase() {
                    return Err(trf!("«{name}» уже существует", "“{name}” already exists"));
                }
                fs::rename(old, &new).map_err(|e| e.to_string())?;
                select = name.to_string();
            }
            InputKind::MkDir if is_remote(&dir) => {
                let mut p = dir.clone();
                for seg in name.split(['/', '\\']).filter(|s| !s.is_empty()) {
                    p = quokka_domain::remote::join(&p, seg).unwrap_or_default();
                }
                if xfer::exists(&p) {
                    return Err(trf!("«{name}» уже существует", "“{name}” already exists"));
                }
                xfer::mkdir_all(&p).map_err(|e| e.to_string())?;
                select = name.split(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(name).to_string();
            }
            InputKind::MkDir => {
                let p = dir.join(name);
                if p.exists() {
                    return Err(trf!("«{name}» уже существует", "“{name}” already exists"));
                }
                fs::create_dir_all(&p).map_err(|e| e.to_string())?;
                select = name.split(['/', '\\']).next().unwrap_or(name).to_string();
            }
            InputKind::NewFile => {
                if name.contains(['/', '\\']) {
                    return Err(tr!("Имя не может содержать косую черту", "Name cannot contain a slash").into());
                }
                if is_remote(&dir) {
                    let p = quokka_domain::remote::join(&dir, name).unwrap_or_default();
                    xfer::create_file(&p).map_err(|e| e.to_string())?;
                } else {
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(dir.join(name))
                        .map_err(|e| e.to_string())?;
                }
                select = name.to_string();
            }
        }
        self.reload_all();
        self.ap().select_name(&select);
        Ok(())
    }

    /// Add an operation to the queue; it starts right away if nothing else is running.
    fn enqueue(&mut self, spec: OpSpec) {
        if let Err(error) = fsops::validate_operation(&spec) { self.toast(error, true); return; }
        let busy = self.queue.is_active();
        let title = spec.title();
        self.queue.push(spec);
        self.ap().marked.clear();
        if busy {
            self.toast(trf!("Добавлено в очередь: {title}", "Added to queue: {title}"), false);
        }
        self.ctx.request_repaint();
    }

    fn poll_queue(&mut self, ctx: &egui::Context) {
        let finished = self.queue.poll(&StdFileOps, &waker(ctx));
        if finished.is_empty() {
            return;
        }
        let dirs: Vec<PathBuf> = finished.iter().flat_map(|f| f.spec.touched_dirs()).collect();
        self.reload_dirs(&dirs);
        for f in &finished {
            if let (OpSpec::Copy { sources, dest, .. }, Ok(_)) = (&f.spec, &f.result) {
                for s in sources {
                    let local = dest.join(quokka_domain::remote::file_name(s).unwrap_or_default());
                    if let Some(i) = self.open_after.iter().position(|p| *p == local) {
                        self.open_after.remove(i);
                        if let Err(err) = fsops::open_external(&local) {
                            self.toast(err.to_string(), true);
                        }
                    }
                }
            }
        }
        for f in finished {
            let error = f.result.is_err() && !f.cancelled;
            let text = if f.cancelled { tr!("Операция отменена", "Operation cancelled").to_string() } else { f.result.unwrap_or_else(|e| e) };
            // even a failed or cancelled recycle may have moved some items already
            let undo = match f.spec {
                OpSpec::Recycle { sources } => {
                    let gone: Vec<PathBuf> = sources.into_iter().filter(|p| !p.exists()).collect();
                    (!gone.is_empty()).then(|| Undo::Recycle(gone))
                }
                // likewise a partly done move can be taken back
                OpSpec::Move { .. } if !f.moved.is_empty() => Some(Undo::Move(f.moved)),
                _ => None,
            };
            let undo = match undo {
                Some(u) => {
                    self.undo = Some(u);
                    true
                }
                None => false,
            };
            self.toast = Some(if undo { Toast { error, ..Toast::undo(text) } } else { Toast::new(text, error) });
        }
    }

    // ───────────────────────────── layout ─────────────────────────────

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
                // each button shows what its F-key does now (it may have been reassigned)
                let short: Vec<(Action, &str)> = fkeys().into_iter().map(|(_, l, a)| (a, l)).collect();
                let slots: Vec<Option<(String, Action)>> = fkeys().iter().map(|(key, _, _)| {
                    hotkeys::with(|m| {
                        let i = m.owner(&hotkeys::Combo::new(false, false, false, *key))?;
                        let a = m.commands[i].action.clone();
                        let label = short.iter().find(|(sa, _)| *sa == a).map_or(m.commands[i].name, |(_, l)| *l);
                        Some((label.to_string(), a))
                    })
                }).collect();
                for (i, slot) in slots.into_iter().enumerate() {
                    let Some((label, a)) = slot else { continue };
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
                    painter.with_clip_rect(r.shrink2(vec2(2.0, 0.0))).text(
                        pos2(badge.right() + 7.0, r.center().y),
                        Align2::LEFT_CENTER,
                        &label,
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
                // vertical divider: one physical pixel on the panel's own right edge, so the file
                // panes drawn next to it cannot cover it (the frame stroke sits half outside)
                let px = 1.0 / ctx.pixels_per_point();
                let edge = ui.max_rect().expand2(vec2(12.0, 14.0));
                ui.painter().vline(edge.right() - px / 2.0, edge.y_range(), Stroke::new(px, BORDER));
                let cur = self.sides[self.active].panel().path.clone();
                let labels = self.settings.sidebar_labels;
                let looks: Vec<Look> = self.settings.places.iter().map(|pl| place_look(&mut self.icon_cache, ctx, pl)).collect();
                let w = ui.available_width();
                let mut op = None;
                let mut side_actions = Vec::new();
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
                            resp.on_hover_text(trf!("{}\nПеретащите, чтобы изменить порядок", "{}\nDrag to reorder", place.name))
                        };
                        if resp.clicked() {
                            self.actions.push(Action::GoTo(place.path.clone()));
                        }
                        resp.context_menu(|ui| {
                            menu_style(ui, 240.0);
                            menu_title(ui, &place.name);
                            if menu_item_icon(ui, Some(Glyph::Pencil), tr!("Изменить название и значок…", "Edit name and icon…"), "", true, false).clicked() {
                                op = Some(PlaceOp::Edit(i));
                                ui.close_menu();
                            }
                            if menu_item_icon(ui, Some(Glyph::ArrowUp), tr!("Переместить выше", "Move up"), "", i > 0, false).clicked() {
                                op = Some(PlaceOp::Up(i));
                                ui.close_menu();
                            }
                            if menu_item_icon(ui, Some(Glyph::ArrowDown), tr!("Переместить ниже", "Move down"), "", i + 1 < n, false).clicked() {
                                op = Some(PlaceOp::Down(i));
                                ui.close_menu();
                            }
                            let local = !place.path.as_os_str().is_empty() && (!is_remote(&place.path) || is_device(&place.path));
                            if cfg!(windows) && local {
                                menu_sep(ui);
                                if menu_item_icon(ui, Some(Glyph::Windows), tr!("Показать меню Windows", "Show Windows menu"), "", true, false).clicked() {
                                    side_actions.push(Action::WindowsMenuOf(place.path.clone()));
                                    ui.close_menu();
                                }
                                if menu_item_icon(ui, Some(Glyph::Info), tr!("Свойства", "Properties"), "", true, false).clicked() {
                                    side_actions.push(Action::PropertiesOf(place.path.clone()));
                                    ui.close_menu();
                                }
                            }
                            menu_sep(ui);
                            if menu_item_icon(ui, Some(Glyph::Trash), tr!("Убрать из избранного", "Remove from favorites"), "", true, true).clicked() {
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
                    let add = side_btn(ui, Look::glyph(Glyph::Plus), false, labels.then_some(tr!("Добавить в избранное", "Add to favorites")), w);
                    let add = if labels { add } else { add.on_hover_text(tr!("Добавить текущую папку в избранное", "Add current folder to favorites")) };
                    if add.clicked() {
                        op = Some(PlaceOp::Add);
                    }
                    if self.settings.show_drives {
                        let (r, _) = ui.allocate_exact_size(vec2(w, 10.0), Sense::hover());
                        ui.painter().line_segment([pos2(r.left() + 6.0, r.center().y), pos2(r.right() - 6.0, r.center().y)], Stroke::new(1.0, BORDER));
                        let mut hovered = None;
                        for d in self.drives.clone() {
                            let info = self.drive(&d);
                            let title = self.drive_title(&d);
                            let kind = info.as_ref().map_or_else(|| quokka_infra::drives::kind(&d), |i| i.kind);
                            let removable = matches!(kind, DriveKind::Removable | DriveKind::Optical);
                            let glyph = match kind {
                                DriveKind::Removable => Glyph::Usb,
                                DriveKind::Device => Glyph::Phone,
                                _ => Glyph::Drive,
                            };
                            let resp = side_btn(ui, Look::glyph(glyph), cur == d, labels.then_some(title.as_str()), w);
                            // a thin usage line along the bottom of the button, as in Explorer's «This PC»
                            if let Some(frac) = info.as_ref().and_then(|i| i.used_fraction()) {
                                let r = resp.rect;
                                let line = if labels {
                                    Rect::from_min_max(pos2(r.left() + 40.0, r.bottom() - 5.0), pos2(r.right() - 10.0, r.bottom() - 2.5))
                                } else {
                                    Rect::from_min_max(pos2(r.left() + 9.0, r.bottom() - 4.5), pos2(r.right() - 9.0, r.bottom() - 2.0))
                                };
                                icons::bar(ui.painter(), line, frac, usage_color(frac));
                            }
                            if !labels && kind != DriveKind::Device {
                                ui.painter().text(
                                    resp.rect.right_bottom() - vec2(5.0, 6.0),
                                    Align2::RIGHT_BOTTOM,
                                    root_label(&d).trim_end_matches(':'),
                                    semibold(10.0),
                                    if cur == d { BLUE } else { TEXT_SOFT },
                                );
                            }
                            if resp.hovered() {
                                hovered = Some(d.clone());
                            }
                            let resp = resp.on_hover_ui(|ui| drive_tooltip(ui, &title, info.as_ref()));
                            if resp.clicked() {
                                self.actions.push(Action::GoTo(d.clone()));
                            }
                            let mut picked = None;
                            resp.context_menu(|ui| {
                                menu_style(ui, 250.0);
                                menu_title(ui, &title);
                                let mut item = |ui: &mut Ui, g: Glyph, text: &str, a: Action| {
                                    if menu_item_icon(ui, Some(g), text, "", true, false).clicked() {
                                        picked = Some(a);
                                        ui.close_menu();
                                    }
                                };
                                item(ui, Glyph::ArrowRight, tr!("Открыть", "Open"), Action::GoTo(d.clone()));
                                item(ui, Glyph::Plus, tr!("Открыть в новой вкладке", "Open in new tab"), Action::OpenInNewTab(d.clone()));
                                item(ui, Glyph::Star, tr!("Добавить в избранное…", "Add to favorites…"), Action::AddFavoritePath(d.clone()));
                                if cfg!(windows) {
                                    menu_sep(ui);
                                    if removable {
                                        item(ui, Glyph::Unpack, tr!("Извлечь", "Eject"), Action::Eject(d.clone()));
                                    }
                                    item(ui, Glyph::Windows, tr!("Показать меню Windows", "Show Windows menu"), Action::WindowsMenuOf(d.clone()));
                                    item(ui, Glyph::Info, tr!("Свойства", "Properties"), Action::PropertiesOf(d.clone()));
                                }
                            });
                            if let Some(a) = picked {
                                self.actions.push(a);
                            }
                        }
                        // while a drive is hovered its numbers are re-read every second
                        if let Some(d) = hovered {
                            let due = self.drive_read_at.get(&d).is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1));
                            if due {
                                self.refresh_drive(&d);
                            }
                            ctx.request_repaint_after(std::time::Duration::from_millis(250));
                        }
                    }
                });
                self.actions.extend(side_actions);
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
            // quick view takes the other pane's place, even in single-pane mode
            if let Some(q) = &mut self.quick_view {
                q.side = 1 - self.active;
            }
            let qv_side = self.quick_view.as_ref().map(|q| q.side);
            if self.settings.dual || qv_side.is_some() {
                let mid = r.center().x.round();
                ui.painter().line_segment([pos2(mid, r.top()), pos2(mid, r.bottom())], Stroke::new(1.0, BORDER));
                let rects = [
                    Rect::from_min_max(r.min, pos2(mid - 0.5, r.max.y)),
                    Rect::from_min_max(pos2(mid + 0.5, r.min.y), r.max),
                ];
                for (i, pr) in rects.into_iter().enumerate() {
                    if Some(i) == qv_side {
                        let panel = self.sides[1 - i].panel();
                        let icons = self.settings.associated_icons.then_some(&mut self.file_icons);
                        if let Some(q) = &mut self.quick_view {
                            #[allow(deprecated)]
                            ui.allocate_ui_at_rect(pr, |ui| q.ui(ui, pr, panel, icons));
                        }
                        continue;
                    }
                    let active = self.active == i;
                    self.pane_rects[i] = Some(pr);
                    #[allow(deprecated)]
                    let out = ui.allocate_ui_at_rect(pr, |ui| pane_ui(ui, &mut self.sides[i], active, i, &mut self.settings.col_widths[i], &mut self.settings.columns[i], self.settings.associated_icons.then_some(&mut self.file_icons))).inner;
                    outs.push((i, out));
                }
            } else {
                let i = self.active;
                self.pane_rects[i] = Some(r);
                #[allow(deprecated)]
                let out = ui.allocate_ui_at_rect(r, |ui| pane_ui(ui, &mut self.sides[i], true, i, &mut self.settings.col_widths[i], &mut self.settings.columns[i], self.settings.associated_icons.then_some(&mut self.file_icons))).inner;
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
        let x_close = take_dialog_close(ctx);
        let esc = x_close || ctx.input(|i| i.key_pressed(Key::Escape));
        let enter = ctx.input(|i| i.key_pressed(Key::Enter));
        let keep = match &mut modal {
            Modal::None => false,
            Modal::Filter { first } => {
                let mut close = esc;
                let mut regex = self.settings.regex;
                let mut case = self.settings.case_sensitive;
                if ctx.input_mut(|i| hotkeys::take_command(&mut i.events, "toggle_regex")) {
                    regex = !regex;
                }
                dialog(ctx, "filter", 540.0_f32.min(ctx.screen_rect().width() - 64.0), |ui| {
                    dialog_header(ui, Glyph::Search, BLUE, tr!("Поиск по имени", "Search by name"), Some(tr!("В текущей папке · Ctrl+F", "In the current folder · Ctrl+F")));
                    let id = self.search_id;
                    let p = self.ap();
                    let (out, _) = jb_field(ui, id, &mut p.filter,
                        tr!("Имя файла или папки…", "File or folder name…"),
                        Glyph::Search, regular(14.0), &[], |ui| {
                            if field_toggle(ui, "Aa", case, tr!("Учитывать регистр", "Match case")).clicked() {
                                case = !case;
                            }
                            if field_toggle(ui, ".*", regex, tr!("Регулярные выражения (Alt+R)", "Regular expressions (Alt+R)")).clicked() {
                                regex = !regex;
                            }
                        });
                    if *first {
                        out.response.request_focus();
                        *first = false;
                    }
                    if out.response.changed() {
                        p.apply_filter();
                    }
                    ui.add_space(8.0);
                    if let Some(error) = &p.filter_error {
                        ui.label(RichText::new(error).color(RED).size(13.0));
                    } else {
                        ui.label(RichText::new(tr!("Фильтр применяется сразу. Очистите поле, чтобы показать все файлы.", "The filter applies immediately. Clear the field to show all files.")).color(MUTED).size(13.0));
                    }
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, tr!("Готово", "Done"), BLUE, Color32::WHITE).clicked() || enter {
                            close = true;
                        }
                        if btn(ui, tr!("Сбросить", "Clear"), INPUT_BG, TEXT).clicked() {
                            p.filter.clear();
                            p.apply_filter();
                            *first = true;
                        }
                    });
                });
                if regex != self.settings.regex || case != self.settings.case_sensitive {
                    self.settings.regex = regex;
                    self.settings.case_sensitive = case;
                    self.sync_settings();
                    self.save_settings();
                }
                !close
            }
            Modal::Help => {
                let mut close = esc || ctx.input_mut(|i| hotkeys::take_command(&mut i.events, "help"));
                let mut edit_keys = false;
                let screen = ctx.screen_rect();
                // header, mouse hint and button take about 230 px of the dialog
                let list_h = (screen.height() - 290.0).max(120.0);
                let width = 640.0_f32.min(screen.width() - 80.0).max(280.0);
                dialog(ctx, "help", width, |ui| {
                    dialog_header(ui, Glyph::Question, BLUE, tr!("Горячие клавиши", "Keyboard shortcuts"), Some(tr!("Quokka полностью управляется с клавиатуры", "Quokka can be fully controlled from the keyboard")));
                    // on a small screen the list scrolls instead of running off the window
                    egui::ScrollArea::vertical().id_salt("help-scroll").max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                        egui::Grid::new("help-grid").num_columns(4).spacing(vec2(10.0, 7.0)).show(ui, |ui| {
                            // reassignable commands with their current keys, then the fixed keys
                            let mut list: Vec<(String, &str)> = hotkeys::with(|m| {
                                m.commands.iter().zip(&m.keys).filter(|(_, k)| !k.is_empty())
                                    .map(|(c, k)| (k.iter().take(2).map(|c| c.label()).collect::<Vec<_>>().join(" / "), c.name)).collect()
                            });
                            list.extend([
                                ("Enter", tr!("Открыть", "Open")),
                                ("Tab", tr!("Другая панель", "Other pane")),
                                ("Backspace", tr!("Папка выше", "Parent folder")),
                                ("Insert", tr!("Отметить и вниз", "Mark and move down")),
                                ("Space", tr!("Отметить", "Mark")),
                                ("Esc", tr!("Снять отметки", "Clear marks")),
                                ("Ctrl+C", tr!("Копировать в буфер", "Copy to clipboard")),
                                ("Ctrl+X", tr!("Вырезать", "Cut")),
                                ("Ctrl+V", tr!("Вставить в текущую папку", "Paste into current folder")),
                            ].map(|(k, d)| (k.to_string(), d)));
                            let half = list.len().div_ceil(2);
                            let rows: Vec<(&str, &str, &str, &str)> = (0..half).map(|i| {
                                let (k1, d1) = (&list[i].0, list[i].1);
                                let (k2, d2) = list.get(half + i).map_or(("", ""), |(k, d)| (k.as_str(), *d));
                                (k1.as_str(), d1, k2, d2)
                            }).collect();
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
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr!("Мышь: Ctrl+клик / Shift+клик — отметить, протянуть рамку с пустого места или со строки — выделить несколько, щелчок по пустому месту — снять выделение, двойной клик — открыть, правый клик — меню.", "Mouse: Ctrl+click / Shift+click — mark, drag a frame from an empty spot or from a row — select several, click on an empty spot — clear selection, double-click — open, right-click — menu.")).size(12.5).color(MUTED));
                    ui.add_space(10.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, tr!("Закрыть", "Close"), BLUE, Color32::WHITE).clicked() {
                            close = true;
                        }
                        if btn(ui, tr!("Изменить сочетания…", "Change shortcuts…"), INPUT_BG, TEXT).clicked() {
                            edit_keys = true;
                        }
                    });
                });
                if edit_keys {
                    self.modal = Modal::Settings { group: SettingsGroup::Keys, filter: String::new() };
                    return;
                }
                !close
            }
            Modal::Columns(editor) => {
                let outcome = editor.show(ctx, esc, enter);
                if outcome == columns::Outcome::Apply {
                    self.settings.columns[editor.pane] = editor.layout.clone();
                    self.save_settings();
                }
                if outcome != columns::Outcome::Open && editor.from_settings {
                    self.modal = Modal::Settings { group: SettingsGroup::View, filter: String::new() };
                }
                outcome == columns::Outcome::Open
            }
            Modal::About => self.about_ui(ctx, esc || enter),
            Modal::Settings { group, filter } => {
                let mut close = esc;
                let hk_id = Id::new("hotkeys-edit");
                let mut hk: hotkeys::EditState = ctx.data(|d| d.get_temp(hk_id)).unwrap_or_default();
                if hk.capturing() {
                    // the recorded keys must not reach text fields or close the dialog
                    close = false;
                    let combo = ctx.input_mut(|i| {
                        let c = hotkeys::captured(&i.events);
                        i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Cut | egui::Event::Copy | egui::Event::Paste(_)));
                        c
                    });
                    if let Some(c) = combo {
                        let mut map = hotkeys::Keymap::with_overrides(&self.settings.hotkeys);
                        hk.on_capture(c, &mut map);
                        self.settings.hotkeys = map.overrides();
                    }
                }
                let before = self.settings.clone();
                let mut op = None;
                let mut reset = false;
                let mut edit_columns = None;
                let mut check_now = false;
                let looks: Vec<Look> = self.settings.places.clone().iter().map(|pl| place_look(&mut self.icon_cache, ctx, pl)).collect();
                let screen = ctx.screen_rect();
                let width = 920.0_f32.min(screen.width() - 60.0);
                let body_h = (screen.height() - 160.0).clamp(340.0, 600.0);
                let q = filter.trim().to_lowercase();
                let visible: Vec<SettingsGroup> = SettingsGroup::all().iter().map(|t| t.0).filter(|g| g.matches(&q)).collect();
                if !visible.is_empty() && !visible.contains(group) {
                    *group = visible[0];
                }
                let sel_id = Id::new("settings-place-sel");
                let mut place_sel: usize = ctx.data(|d| d.get_temp(sel_id)).unwrap_or(0);
                flat_dialog(ctx, "settings", width, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    let (body, _) = ui.allocate_exact_size(vec2(width, body_h), Sense::hover());
                    let side = Rect::from_min_size(body.min, vec2(SETTINGS_NAV_W, body_h));
                    let content = Rect::from_min_max(pos2(side.right() + 1.0, body.top()), body.max);
                    ui.painter().rect_filled(side, Rounding { nw: FLAT_R, ..Default::default() }, NAV_BG);
                    ui.painter().vline(side.right(), side.y_range(), Stroke::new(1.0, BORDER));

                    // tree with its search box
                    place(ui, egui::UiBuilder::new().max_rect(side.shrink2(vec2(14.0, 18.0))), |ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
                        let has_text = !filter.is_empty();
                        let mut clear = false;
                        jb_field(ui, Id::new("settings-filter"), filter, tr!("Поиск настроек", "Search settings"), Glyph::Search, regular(13.5), &[], |ui| {
                            clear = has_text && jb_tool(ui, Glyph::Close, true, tr!("Очистить", "Clear")).clicked();
                        });
                        if clear {
                            filter.clear();
                        }
                        ui.add_space(14.0);
                        settings_nav(ui, group, &visible);
                        if visible.is_empty() {
                            ui.add_space(8.0);
                            ui.label(RichText::new(tr!("Ничего не найдено", "Nothing found")).size(13.0).color(MUTED));
                        }
                    });

                    // page
                    place(ui, egui::UiBuilder::new().max_rect(content.shrink2(vec2(32.0, 20.0))), |ui| {
                        let name = SettingsGroup::all().iter().find(|t| t.0 == *group).map_or("", |t| t.1);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.label(RichText::new(tr!("Настройки", "Settings")).size(13.5).color(MUTED));
                            ui.label(RichText::new("›").size(14.0).color(MUTED));
                            ui.label(RichText::new(name).font(semibold(14.0)).color(TEXT));
                        });
                        ui.add_space(8.0);
                        if visible.is_empty() {
                            return;
                        }
                        egui::ScrollArea::vertical()
                            .id_salt(("settings-content", *group as u8))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width() - 8.0);
                                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                                let st = &mut self.settings;
                                match group {
                                    SettingsGroup::View => {
                                        jb_group(ui, tr!("Язык интерфейса", "Interface language"));
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(tr!("Язык:", "Language:")).size(13.5).color(TEXT));
                                            ui.add_space(8.0);
                                            egui::ComboBox::from_id_salt("settings-language")
                                                .selected_text(st.language.native_name())
                                                .width(160.0)
                                                .show_ui(ui, |ui| {
                                                    for lang in Lang::ALL {
                                                        ui.selectable_value(&mut st.language, lang, lang.native_name());
                                                    }
                                                });
                                        });
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        jb_comment(ui, 0.0, tr!("Русский или английский. Применяется сразу.", "English or Russian. Applied immediately."));
                                        ui.spacing_mut().item_spacing.y = 6.0;
                                        jb_group(ui, tr!("Файлы", "Files"));
                                        jb_option(ui, &mut st.show_hidden, tr!("Показывать скрытые файлы", "Show hidden files"), tr!("Файлы и папки с атрибутом «скрытый» или начинающиеся с точки. Клавиша F11.", "Files and folders with the “hidden” attribute or starting with a dot. Key F11."), &q);
                                        if cfg!(windows) {
                                            jb_option(ui, &mut st.associated_icons, tr!("Иконки связанных приложений", "Associated application icons"), tr!("Значки Windows для файлов в панелях и результатах поиска. Если значок недоступен, используется встроенный.", "Windows icons for files in panes and search results. If an icon is unavailable, the built-in one is used."), &q);
                                        }
                                        jb_group(ui, tr!("Колонки файловых панелей", "File pane columns"));
                                        ui.horizontal(|ui| {
                                            if jb_btn(ui, tr!("Колонки левой панели…", "Left pane columns…"), false).clicked() { edit_columns = Some(0); }
                                            if jb_btn(ui, tr!("Колонки правой панели…", "Right pane columns…"), false).clicked() { edit_columns = Some(1); }
                                        });
                                        jb_comment(ui, 0.0, tr!("В отдельном окне: перетащите колонки для изменения порядка, флажки управляют видимостью.", "In a separate dialog: drag columns to reorder, use checkboxes to show or hide them."));
                                        jb_group(ui, tr!("Окно", "Window"));
                                        jb_option(ui, &mut st.dual, tr!("Две панели", "Two panes"), tr!("Показывать две файловые панели рядом. Клавиша F12.", "Show two file panes side by side. Key F12."), &q);
                                        jb_option(ui, &mut st.restore_session, tr!("Открывать папки прошлого сеанса", "Reopen folders from the last session"), tr!("При запуске вернуть вкладки обеих панелей и активную панель, как было при выходе.", "On startup, restore the tabs of both panes and the active pane as they were on exit."), &q);
                                        jb_option(ui, &mut st.multi_instance, tr!("Разрешить несколько окон Quokka", "Allow multiple Quokka windows"), tr!("Если выключено, повторный запуск не открывает второе окно, а показывает уже запущенное (папка из командной строки откроется в нём новой вкладкой).", "When off, launching again does not open a second window but shows the running one (a folder from the command line opens there in a new tab)."), &q);
                                        if cfg!(windows) {
                                            jb_group(ui, tr!("Область уведомлений (трей)", "Notification area (tray)"));
                                            jb_option(ui, &mut st.tray_icon, tr!("Значок в трее", "Tray icon"), tr!("Щелчок по значку показывает окно, правый щелчок — меню с выходом.", "Clicking the icon shows the window, right-clicking opens a menu with Exit."), &q);
                                            jb_option(ui, &mut st.close_to_tray, tr!("Скрывать в трей при закрытии", "Hide to tray on close"), tr!("Крестик окна прячет Quokka в трей, а не завершает работу; фоновые операции продолжаются. Выход — F10 или «Выход» в меню значка. Значок в трее при этом показывается всегда.", "The window’s close button hides Quokka to the tray instead of quitting; background operations continue. Exit — F10 or “Exit” in the icon menu. The tray icon is always shown in this case."), &q);
                                        }
                                        jb_group(ui, tr!("Обновления", "Updates"));
                                        jb_option(ui, &mut st.check_updates, tr!("Проверять обновления при запуске", "Check for updates at startup"), tr!("Раз в сутки Quokka спрашивает GitHub о новой версии и предлагает обновиться. Ничего не скачивается без вашего согласия.", "Once a day Quokka asks GitHub for a new version and offers to update. Nothing is downloaded without your consent."), &q);
                                        if jb_btn(ui, tr!("Проверить сейчас", "Check now"), false).clicked() {
                                            check_now = true;
                                        }
                                        ui.add_space(10.0);
                                        jb_group(ui, tr!("Внешний редактор", "External editor"));
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(tr!("Команда (F4):", "Command (F4):")).size(13.5).color(TEXT));
                                            jb_field(ui, Id::new("settings-editor"), &mut st.editor, &fsops::default_editor_hint(), Glyph::Code, mono(13.0), &[], |_| {});
                                        });
                                        jb_comment(
                                            ui,
                                            0.0,
                                            tr!("Программа или команда; %f заменяется путём к файлу, иначе путь добавляется в конец. Пусто — редактор по умолчанию. Встроенный редактор — Shift+F4.", "Program or command; %f is replaced with the file path, otherwise the path is appended. Empty — default editor. Built-in editor — Shift+F4."),
                                        );
                                    }
                                    SettingsGroup::Search => {
                                        jb_group(ui, tr!("Фильтр по имени (Ctrl+F)", "Filter by name (Ctrl+F)"));
                                        jb_option(
                                            ui,
                                            &mut st.regex,
                                            tr!("Регулярные выражения", "Regular expressions"),
                                            tr!("Например: ^IMG_\\d+\\.jpe?g$ или (отчёт|report).*\\.pdf. Переключатель: Alt+R или «.*» в окне поиска.", "For example: ^IMG_\\d+\\.jpe?g$ or (report|отчёт).*\\.pdf. Toggle: Alt+R or “.*” in the search dialog."),
                                            &q,
                                        );
                                        jb_option(ui, &mut st.case_sensitive, tr!("Учитывать регистр букв", "Match letter case"), tr!("«Отчёт» и «отчёт» будут считаться разными. Кнопка «Aa» в окне поиска.", "“Report” and “report” will be treated as different. The “Aa” button in the search dialog."), &q);
                                    }
                                    SettingsGroup::Keys => {
                                        hotkeys::page(ui, &mut st.hotkeys, &mut hk, &q);
                                    }
                                    SettingsGroup::Sidebar => {
                                        jb_group(ui, tr!("Боковое меню", "Sidebar"));
                                        jb_option(ui, &mut st.sidebar_labels, tr!("Показывать подписи", "Show labels"), tr!("Широкое меню с названиями вместо одних значков.", "Wide menu with names instead of icons only."), &q);
                                        jb_option(ui, &mut st.show_drives, tr!("Показывать диски", "Show drives"), tr!("Список дисков под закладками.", "List of drives below the bookmarks."), &q);
                                        jb_group(ui, tr!("Избранное", "Favorites"));
                                        let n = st.places.len();
                                        place_sel = place_sel.min(n.saturating_sub(1));
                                        egui::Frame::none().stroke(Stroke::new(1.0, FIELD_BORDER)).rounding(5.0).show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 2.0;
                                                ui.add_space(4.0);
                                                ui.set_height(32.0);
                                                if jb_tool(ui, Glyph::Plus, true, tr!("Добавить текущую папку", "Add current folder")).clicked() {
                                                    op = Some(PlaceOp::Add);
                                                }
                                                if jb_tool(ui, Glyph::Minus, n > 0, tr!("Убрать", "Remove")).clicked() {
                                                    op = Some(PlaceOp::Remove(place_sel));
                                                }
                                                if jb_tool(ui, Glyph::Pencil, n > 0, tr!("Изменить", "Edit")).clicked() {
                                                    op = Some(PlaceOp::Edit(place_sel));
                                                }
                                                if jb_tool(ui, Glyph::ArrowUp, place_sel > 0, tr!("Выше", "Up")).clicked() {
                                                    op = Some(PlaceOp::Up(place_sel));
                                                    place_sel -= 1;
                                                }
                                                if jb_tool(ui, Glyph::ArrowDown, place_sel + 1 < n, tr!("Ниже", "Down")).clicked() {
                                                    op = Some(PlaceOp::Down(place_sel));
                                                    place_sel += 1;
                                                }
                                            });
                                            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                                            ui.painter().rect_filled(line, 0.0, BORDER);
                                            for (i, pl) in st.places.iter().enumerate() {
                                                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                                                let p = ui.painter();
                                                if i == place_sel {
                                                    p.rect_filled(r.shrink(2.0), 4.0, NAV_SEL);
                                                } else if resp.hovered() {
                                                    p.rect_filled(r.shrink(2.0), 4.0, HOVER);
                                                }
                                                draw_look(p, Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), vec2(18.0, 18.0)), looks[i], SLATE);
                                                let pc = p.with_clip_rect(r.shrink2(vec2(8.0, 0.0)));
                                                let end = pc.text(pos2(r.left() + 36.0, r.center().y), Align2::LEFT_CENTER, &pl.name, regular(13.5), TEXT).right();
                                                let path = if pl.path.as_os_str().is_empty() { tr!("Этот компьютер", "This PC").to_string() } else { pl.path.display().to_string() };
                                                pc.text(pos2(end + 12.0, r.center().y), Align2::LEFT_CENTER, &path, regular(12.5), MUTED);
                                                let resp = resp.on_hover_text(&path);
                                                if resp.clicked() {
                                                    place_sel = i;
                                                }
                                                if resp.double_clicked() {
                                                    op = Some(PlaceOp::Edit(i));
                                                }
                                            }
                                            ui.add_space(2.0);
                                        });
                                        jb_comment(ui, 0.0, tr!("Порядок можно менять и перетаскиванием прямо в боковом меню. Двойной щелчок — изменить.", "The order can also be changed by dragging right in the sidebar. Double-click — edit."));
                                        ui.add_space(4.0);
                                        if ui.link(RichText::new(tr!("Восстановить избранное по умолчанию", "Restore default favorites")).size(13.0).color(BLUE)).clicked() {
                                            reset = true;
                                        }
                                    }
                                }
                            });
                    });

                    // footer
                    let (foot, _) = ui.allocate_exact_size(vec2(width, 62.0), Sense::hover());
                    ui.painter().hline(foot.x_range(), foot.top(), Stroke::new(1.0, BORDER));
                    place(ui, egui::UiBuilder::new().max_rect(foot.shrink2(vec2(28.0, 0.0))).layout(Layout::left_to_right(Align::Center)), |ui| {
                        icons::glyph(ui.painter(), Rect::from_center_size(pos2(ui.cursor().left() + 9.0, foot.center().y), vec2(16.0, 16.0)), Glyph::Question, MUTED);
                        ui.add_space(26.0);
                        ui.label(RichText::new(tr!("Изменения применяются и сохраняются сразу", "Changes are applied and saved immediately")).size(12.5).color(MUTED));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if jb_btn(ui, tr!("Готово", "Done"), true).clicked() {
                                close = true;
                            }
                        });
                    });
                });
                ctx.data_mut(|d| d.insert_temp(sel_id, place_sel));
                if close || *group != SettingsGroup::Keys {
                    hk = hotkeys::EditState::default();
                }
                ctx.data_mut(|d| d.insert_temp(hk_id, hk));
                if self.settings.hotkeys != before.hotkeys {
                    hotkeys::install(hotkeys::Keymap::with_overrides(&self.settings.hotkeys));
                }
                if reset {
                    self.settings.places = config::default_places();
                }
                if self.settings.language != before.language {
                    quokka_domain::lang::set(self.settings.language);
                    config::relabel_standard_places(&mut self.settings.places);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(crate::title().into()));
                    // file type names are made while listing a folder
                    self.reload_all();
                }
                if self.settings != before {
                    self.sync_settings();
                    self.save_settings();
                }
                if let Some(pane) = edit_columns {
                    self.modal = Modal::Columns(columns::ColumnsDialog::new(pane, self.settings.columns[pane].clone(), true));
                    return;
                }
                if check_now {
                    self.check_updates_now();
                    return;
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
                let title = if ed.index.is_some() { tr!("Изменить избранное", "Edit favorite") } else { tr!("Добавить в избранное", "Add to favorites") };
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
                                None => tr!("Стандартный значок", "Default icon").into(),
                            };
                            ui.label(RichText::new(what).size(13.0).color(MUTED));
                        });
                    });
                    ui.add_space(12.0);

                    ui.columns(2, |cols| {
                        cols[0].label(RichText::new(tr!("Название", "Name")).color(TEXT_SOFT).size(13.0));
                        let r = cols[0].add(egui::TextEdit::singleline(&mut ed.name).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)).font(regular(15.0)));
                        if ed.first {
                            ed.first = false;
                            r.request_focus();
                        }
                        if r.lost_focus() && enter {
                            submit = true;
                        }
                        cols[1].label(RichText::new(tr!("Папка (пусто — «Этот компьютер»)", "Folder (empty — “This PC”)")).color(TEXT_SOFT).size(13.0));
                        let r = cols[1].add(egui::TextEdit::singleline(&mut ed.path).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)).font(regular(14.0)));
                        if r.lost_focus() && enter {
                            submit = true;
                        }
                    });
                    ui.add_space(10.0);

                    // source tabs
                    ui.horizontal(|ui| {
                        for (t, label) in [(IconTab::Builtin, tr!("Стандартные", "Standard")), (IconTab::Windows, tr!("Значки Windows", "Windows icons")), (IconTab::File, tr!("Свой файл", "Custom file"))] {
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
                                ui.label(RichText::new(tr!("Цвет значка", "Icon color")).color(TEXT_SOFT).size(13.0));
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    for (c, name) in icon_colors() {
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
                                    if ui.color_edit_button_srgb(&mut rgb).on_hover_text(tr!("Любой цвет", "Any color")).changed() {
                                        ed.color = Some(rgb);
                                        ed.icon = None;
                                    }
                                });
                            }
                            IconTab::Windows => {
                                ui.horizontal(|ui| {
                                    let cur = ed.lib.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                                    let label = iconsrc::windows_libs().iter().find(|(_, f)| *f == cur).map(|(l, _)| *l).unwrap_or(tr!("Библиотека", "Library"));
                                    egui::ComboBox::from_id_salt("icon-lib").selected_text(label).width(260.0).show_ui(ui, |ui| {
                                        for (l, f) in iconsrc::windows_libs() {
                                            if ui.selectable_label(cur == f, l).clicked() {
                                                ed.lib = iconsrc::system_lib(f);
                                                ed.lib_count = None;
                                                ed.page = 0;
                                            }
                                        }
                                    });
                                    let on = ed.icon.as_deref() == Some(iconsrc::SHELL);
                                    if pill(ui, tr!("Значок самой папки", "The folder’s own icon"), on).on_hover_text(tr!("Тот значок, что показывает Проводник (учитывает desktop.ini)", "The icon Explorer shows (respects desktop.ini)")).clicked() {
                                        ed.icon = Some(iconsrc::SHELL.into());
                                    }
                                });
                                ui.add_space(8.0);
                                resource_grid(ui, cache, ctx, ed);
                            }
                            IconTab::File => {
                                ui.horizontal(|ui| {
                                    if jb_btn(ui, tr!("Выбрать файл…", "Choose file…"), false).clicked() {
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
                                    ui.label(RichText::new(tr!("ico, png, jpg, svg, bmp, gif, webp, а также exe и dll", "ico, png, jpg, svg, bmp, gif, webp, as well as exe and dll")).size(12.5).color(MUTED));
                                });
                                ui.add_space(8.0);
                                let own_lib = iconsrc::is_resource_file(&ed.lib)
                                    && !ed.lib.starts_with(std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_default());
                                if own_lib {
                                    ui.label(RichText::new(trf!("Значки из {}", "Icons from {}", ed.lib.display())).size(12.5).color(TEXT_SOFT));
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
                                    ui.label(RichText::new(tr!("Файл не выбран", "No file selected")).color(MUTED));
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
                        if btn(ui, tr!("Сохранить", "Save"), BLUE, Color32::WHITE).clicked() {
                            submit = true;
                        }
                        if btn(ui, tr!("Отмена", "Cancel"), INPUT_BG, TEXT).clicked() {
                            cancel = true;
                        }
                    });
                });
                let mut done = cancel;
                if submit {
                    let p = PathBuf::from(ed.path.trim());
                    if ed.name.trim().is_empty() {
                        ed.error = Some(tr!("Введите название", "Enter a name").into());
                    } else if !p.as_os_str().is_empty() && !p.is_dir() {
                        ed.error = Some(tr!("Такой папки нет", "No such folder").into());
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
                    self.modal = Modal::Settings { group: SettingsGroup::Sidebar, filter: String::new() };
                }
                !done
            }
            Modal::Input { kind, title, label, text, first, error } => {
                let mut submit = false;
                let mut cancel = esc;
                let (glyph, color) = match kind {
                    InputKind::Selection(_) => (Glyph::Check, BLUE),
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
                        if btn(ui, tr!("ОК", "OK"), BLUE, Color32::WHITE).clicked() {
                            submit = true;
                        }
                        if btn(ui, tr!("Отмена", "Cancel"), INPUT_BG, TEXT).clicked() {
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
                let extracting = sources.iter().any(|s| quokka_infra::archive::is_member(s));
                let (title, glyph, color) = if extracting { (tr!("Извлечь", "Extract"), Glyph::Unpack, BLUE) } else if *mv { (tr!("Переместить", "Move"), Glyph::ArrowRight, GREEN) } else { (tr!("Копировать", "Copy"), Glyph::Copy, BLUE) };
                dialog(ctx, "transfer", 520.0, |ui| {
                    dialog_header(ui, glyph, color, &format!("{title} {what}"), Some(tr!("Папка назначения", "Destination folder")));
                    let r = ui.add(egui::TextEdit::singleline(dest).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)).font(regular(15.0)));
                    if *first {
                        *first = false;
                        r.request_focus();
                    }
                    if r.lost_focus() && enter {
                        submit = true;
                    }
                    ui.add_space(8.0);
                    ui.checkbox(overwrite, tr!("Заменять существующие файлы", "Overwrite existing files"));
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, title, color, Color32::WHITE).clicked() {
                            submit = true;
                        }
                        if btn(ui, tr!("Отмена", "Cancel"), INPUT_BG, TEXT).clicked() {
                            cancel = true;
                        }
                    });
                });
                if submit {
                    let mut d = PathBuf::from(dest.trim());
                    if quokka_infra::archive::location(&d).is_some() {
                        self.toast(tr!("Выберите обычную папку назначения вне архива", "Choose a regular destination folder outside the archive"), true);
                        true
                    } else if d.as_os_str().is_empty() {
                        self.toast(tr!("Укажите папку назначения", "Specify the destination folder"), true);
                        true
                    } else {
                        if is_remote(&d) {
                            d = quokka_domain::remote::normalize(&d).unwrap_or(d);
                        } else if d.is_relative() {
                            d = match quokka_domain::remote::join(&self.ap_ref().path, &d.to_string_lossy()) {
                                Some(r) => r,
                                None => self.ap_ref().path.join(d),
                            };
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
                        (tr!("Удалить навсегда", "Delete permanently"), trf!("{n} будет удалено без возможности восстановления", "{n} will be deleted permanently"))
                    } else {
                        (tr!("Удалить в корзину", "Move to Recycle Bin"), trf!("{n} будет перемещено в корзину Windows", "{n} will be moved to the Windows Recycle Bin"))
                    };
                    dialog_header(ui, Glyph::Trash, RED, title, Some(&sub));
                    for s in sources.iter().take(6) {
                        let name = s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        ui.label(RichText::new(format!("•  {name}")).color(TEXT));
                    }
                    if sources.len() > 6 {
                        ui.label(RichText::new(trf!("…и ещё {}", "…and {} more", sources.len() - 6)).color(MUTED));
                    }
                    ui.add_space(6.0);
                    let note = if permanent {
                        tr!("Это действие нельзя отменить.", "This action cannot be undone.")
                    } else {
                        tr!("Файлы можно будет восстановить из корзины. Shift+Del — удалить навсегда.", "Files can be restored from the Recycle Bin. Shift+Del — delete permanently.")
                    };
                    ui.label(RichText::new(note).color(MUTED).size(13.0));
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, if permanent { tr!("Удалить", "Delete") } else { tr!("В корзину", "To Recycle Bin") }, RED, Color32::WHITE).clicked() {
                            submit = true;
                        }
                        if btn(ui, tr!("Отмена", "Cancel"), INPUT_BG, TEXT).clicked() {
                            cancel = true;
                        }
                    });
                });
                if submit && !cancel {
                    let sources = std::mem::take(sources);
                    self.undo = None;
                    self.enqueue(if permanent { OpSpec::Delete { sources } } else { OpSpec::Recycle { sources } });
                    false
                } else {
                    !cancel
                }
            }
            Modal::Preview(viewer) => viewer.show(ctx, esc),
            Modal::Editor { path, text, original, confirm_close } => {
                let dirty = text != original;
                let mut save = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::S) || i.consume_key(Modifiers::NONE, Key::F2));
                let mut close = esc || ctx.input(|i| i.key_pressed(Key::F10));
                let mut discard = false;
                let screen = ctx.screen_rect();
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let sub = if dirty { tr!("Изменён — Ctrl+S, чтобы сохранить", "Modified — Ctrl+S to save").to_string() } else { remote::shown(path) };
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
                            if btn(ui, tr!("Сохранить и закрыть", "Save and close"), BLUE, Color32::WHITE).clicked() {
                                save = true;
                                discard = true;
                            }
                            if btn(ui, tr!("Не сохранять", "Don’t save"), RED, Color32::WHITE).clicked() {
                                discard = true;
                                *original = text.clone();
                            }
                            if btn(ui, tr!("Отмена", "Cancel"), INPUT_BG, TEXT).clicked() {
                                *confirm_close = false;
                            }
                            ui.label(RichText::new(tr!("Есть несохранённые изменения", "There are unsaved changes")).color(ORANGE));
                        } else {
                            if btn(ui, tr!("Закрыть", "Close"), INPUT_BG, TEXT).clicked() {
                                close = true;
                            }
                            if btn(ui, tr!("Сохранить", "Save"), BLUE, Color32::WHITE).clicked() {
                                save = true;
                            }
                        }
                    });
                });
                let mut keep = true;
                if save {
                    match quokka_infra::xfer::write_all(&*path, text.as_bytes()) {
                        Ok(()) => {
                            *original = text.clone();
                            self.toast(trf!("Сохранено: {name}", "Saved: {name}"), false);
                            self.reload_all();
                        }
                        Err(e) => {
                            self.toast(trf!("Не удалось сохранить: {e}", "Failed to save: {e}"), true);
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
                    Some(_) => trf!("Распаковать «{}»", "Extract “{}”", archives[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                    None => trf!("Распаковать {}", "Extract {}", items_text(archives.len() as u64)),
                };
                dialog(ctx, "unzip", 520.0, |ui| {
                    dialog_header(ui, Glyph::Archive, ORANGE, &title, Some(tr!("Папка назначения", "Destination folder")));
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
                        Some(stem) => trf!("Создать папку «{stem}»", "Create folder “{stem}”"),
                        None => tr!("Каждый архив — в отдельную папку", "Each archive into its own folder").to_string(),
                    };
                    ui.checkbox(subfolder, sub);
                    ui.checkbox(overwrite, tr!("Заменять существующие файлы", "Overwrite existing files"));
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, tr!("Распаковать", "Extract"), ORANGE, Color32::WHITE).clicked() {
                            submit = true;
                        }
                        if btn(ui, tr!("Отмена", "Cancel"), INPUT_BG, TEXT).clicked() {
                            cancel = true;
                        }
                    });
                });
                if submit {
                    let mut d = PathBuf::from(dest.trim());
                    if d.as_os_str().is_empty() {
                        self.toast(tr!("Укажите папку назначения", "Specify the destination folder"), true);
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
                    dialog_header(ui, Glyph::Question, ORANGE, tr!("Операции ещё выполняются", "Operations are still running"), Some(&trf!("Незавершённых операций: {n}", "Unfinished operations: {n}")));
                    ui.label(RichText::new(tr!("Если выйти сейчас, текущая операция будет прервана, а ожидающие — отменены.", "If you exit now, the current operation will be aborted and pending ones cancelled.")).color(TEXT));
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, tr!("Прервать и выйти", "Abort and exit"), RED, Color32::WHITE).clicked() {
                            quit = true;
                        }
                        if btn(ui, tr!("Остаться", "Stay"), INPUT_BG, TEXT).clicked() {
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
            Modal::Find(fd) => self.find_ui(ctx, fd, esc, x_close, enter),
            Modal::MultiRename(d) => self.multi_rename_ui(ctx, d, esc, enter),
            Modal::Connect(d) => self.connect_ui(ctx, d, esc, enter),
            Modal::Update(d) => self.update_ui(ctx, d, esc),
            Modal::Convert(d) => self.convert_ui(ctx, d, esc, enter),
            Modal::Message { title, text } => {
                let mut close = esc || enter;
                dialog(ctx, "message", 420.0, |ui| {
                    dialog_header(ui, Glyph::Question, ORANGE, title, None);
                    ui.label(RichText::new(text.as_str()).color(TEXT));
                    ui.add_space(14.0);
                    buttons_row(ui, |ui| {
                        if btn(ui, tr!("ОК", "OK"), BLUE, Color32::WHITE).clicked() {
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
        let mut pressed = None;
        let Some(t) = &mut self.toast else { return };
        let born = *t.born.get_or_insert(now);
        let life = if t.button.is_some() { 8.0 } else if t.error { 4.5 } else { 2.5 };
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
            .interactable(t.button.is_some())
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
                            if let Some((label, action)) = &t.button {
                                ui.add_space(10.0);
                                let b = sized_btn(ui, label, Color32::from_white_alpha(28), BLUE_SOFT, Stroke::NONE);
                                let b = if *action == Action::UndoDelete { b.on_hover_text(tr!("Отменить (Ctrl+Z)", "Undo (Ctrl+Z)")) } else { b };
                                if b.clicked() {
                                    pressed = Some(action.clone());
                                }
                            }
                        });
                    });
            });
        if let Some(a) = pressed {
            self.toast = None;
            self.actions.push(a);
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
        let typing = self.text_focus(ctx);
        let allow = self.idle() && !typing;
        dedupe_paste(&mut raw.events, down, typing, allow, &mut self.paste_latch, std::time::Instant::now());
        if down || self.paste_latch.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
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
        self.tray_frame(ctx);
        let close = ctx.input(|i| i.viewport().close_requested());
        if close && !self.allow_close && self.settings.close_to_tray && !self.exit_requested && crate::tray::icon_shown() {
            // the window cross hides to the tray; F10 and «Выход» in the tray menu quit
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.save_session();
            crate::tray::hide_window();
        } else if close && !self.allow_close && self.queue.is_active() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if matches!(self.modal, Modal::None) {
                self.modal = Modal::ConfirmQuit;
            } else {
                self.toast(tr!("Идут операции — закройте диалог, чтобы выйти", "Operations are running — close the dialog to exit"), true);
            }
        }
        if close {
            self.exit_requested = false;
        }
        self.poll_queue(ctx);
        self.poll_updates();
        self.poll_drives();
        let opening = ctx.data(|d| d.get_temp::<OpenFeedback>(open_feedback_id()));
        if let Some(opening) = opening {
            if ctx.input(|i| i.time) - opening.started >= OPEN_FEEDBACK_SECS {
                ctx.data_mut(|d| d.remove::<OpenFeedback>(open_feedback_id()));
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        for si in 0..2 {
            for ti in 0..self.sides[si].tabs.len() {
                let shown = self.shown(si, ti);
                self.sides[si].tabs[ti].poll(shown);
            }
        }
        if self.quitting && !self.queue.is_active() {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.idle() {
            self.handle_keys(ctx);
            // Keyboard commands run before drawing, so this very frame shows the moved
            // cursor. Deferred to the end of the frame they used to show up only on the
            // next input event: egui removes a consumed key from the frame's events and
            // then sees no reason to paint again, so the list lagged a key press behind.
            for a in std::mem::take(&mut self.actions) {
                self.run(a, ctx);
            }
        }
        self.menu_bar(ctx);
        self.fkey_bar(ctx);
        self.sidebar(ctx);
        self.central(ctx);
        self.drag_and_drop(ctx);
        self.modal_ui(ctx);
        self.queue_ui(ctx);
        self.toast_ui(ctx);
        backdrop(ctx, !matches!(self.modal, Modal::None));
        let actions = std::mem::take(&mut self.actions);
        if !actions.is_empty() {
            // the result of clicks and menu commands is drawn in the next frame
            ctx.request_repaint();
        }
        for a in actions {
            self.run(a, ctx);
        }
        self.focus_policy(ctx);
        self.save_session();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_session();
        crate::tray::set_icon(false);
    }
}

/// Folder to reopen for a saved tab: the nearest one that still exists. Network paths
/// are kept as they are — checking a dead share could hang the start.
fn restorable(path: &std::path::Path) -> PathBuf {
    if path.as_os_str().is_empty() || path.to_string_lossy().starts_with(r"\\") {
        return path.to_path_buf();
    }
    if quokka_infra::archive::location(path).is_some() { return path.to_path_buf(); }
    if is_remote(path) { return PathBuf::new(); }
    path.ancestors().find(|p| p.is_dir()).map(PathBuf::from).unwrap_or_default()
}

// ───────────────────────────── pane ─────────────────────────────

struct Cols {
    name: (f32, f32),
    modified: Option<(f32, f32)>,
    size: Option<(f32, f32)>,
    kind: Option<(f32, f32)>,
    extra: [Option<(f32, f32)>; 4],
}

/// Leave room for the file icon when the pane is narrower than its columns.
const MIN_NAME_W: f32 = 48.0;

impl Cols {
    fn content_width(width: f32, widths: &[f32; config::COLUMN_COUNT - 1], layout: &config::ColumnLayout) -> f32 {
        let used: f32 = (1..config::COLUMN_COUNT).filter(|id| layout.visible[*id]).map(|id| widths[id - 1]).sum();
        width.max(used + MIN_NAME_W)
    }

    #[cfg(test)]
    fn new(r: Rect, widths: &[f32; config::COLUMN_COUNT - 1]) -> Self {
        Self::layout(r, widths, &config::ColumnLayout::default())
    }

    fn layout(r: Rect, widths: &[f32; config::COLUMN_COUNT - 1], layout: &config::ColumnLayout) -> Self {
        let visible = layout.visible;
        let used: f32 = (1..config::COLUMN_COUNT).filter(|id| visible[*id]).map(|id| widths[id - 1]).sum();
        let mut cells = [None; config::COLUMN_COUNT];
        let mut left = r.left();
        for id in layout.order {
            if id == 0 || visible[id] {
                let width = if id == 0 { (r.width() - used).max(MIN_NAME_W) } else { widths[id - 1] };
                cells[id] = Some((left, left + width));
                left += width;
            }
        }
        Self { name: cells[0].unwrap(), modified: cells[1], size: cells[2], kind: cells[3], extra: [cells[4], cells[5], cells[6], cells[7]] }
    }

    fn cell(&self, id: usize) -> Option<(f32, f32)> {
        match id { 0 => Some(self.name), 1 => self.modified, 2 => self.size, 3 => self.kind, _ => self.extra[id - 4] }
    }
}

fn pane_ui(ui: &mut Ui, side: &mut Side, active: bool, idx: usize, col_widths: &mut [f32; config::COLUMN_COUNT - 1], layout: &mut config::ColumnLayout, file_icons: Option<&mut crate::file_icons::FileIcons>) -> PaneOut {
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
    table(ui, panel, active, idx, table_r, col_widths, layout, &mut out, file_icons);
    out.zone.area = full;
    footer(ui, panel, foot_r);
    out
}

fn tab_strip(ui: &mut Ui, side: &mut Side, active: bool, idx: usize, rect: Rect, out: &mut PaneOut) {
    const GAP: f32 = 4.0;
    const MIN_W: f32 = 100.0;
    const MAX_W: f32 = 200.0;
    const TOP: f32 = 5.0;
    let p = ui.painter().clone();
    p.line_segment([pos2(rect.left(), rect.bottom() - 0.5), pos2(rect.right(), rect.bottom() - 0.5)], Stroke::new(1.0, BORDER));
    let n = side.tabs.len();
    let cy = rect.top() + TOP + (rect.height() - TOP) / 2.0;

    // Tabs shrink to fit; below MIN_W the strip scrolls instead of spilling
    // into the neighbouring pane. The "+" (and the tab list) stay pinned on the right.
    let full = rect.width() - 10.0 - 36.0;
    let fit_w = full / n as f32 - GAP;
    let overflow = fit_w < MIN_W;
    let tab_w = fit_w.clamp(MIN_W, MAX_W);
    let buttons_w = if overflow { 62.0 } else { 36.0 };
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
        let tr = Rect::from_min_max(pos2(x, rect.top() + TOP), pos2(x + tab_w, rect.bottom()));
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
        pc.rect_filled(tr, Rounding { nw: 7.0, ne: 7.0, sw: 0.0, se: 0.0 }, fill);
        icons::folder(&pc, Rect::from_center_size(pos2(tr.left() + 16.0, tr.center().y), vec2(16.0, 14.0)));
        let text_clip = Rect::from_min_max(tr.min, pos2(tr.right() - if n > 1 { 26.0 } else { 6.0 }, tr.bottom())).intersect(strip);
        p.with_clip_rect(text_clip).text(
            pos2(tr.left() + 30.0, tr.center().y),
            Align2::LEFT_CENTER,
            side.tabs[i].title(),
            if is_cur { semibold(13.0) } else { regular(13.0) },
            if is_cur { TEXT } else { TEXT_SOFT },
        );
        let resp = resp.on_hover_text(remote::shown(&side.tabs[i].path));
        if n > 1 {
            let cr = Rect::from_center_size(pos2(tr.right() - 14.0, tr.center().y), vec2(18.0, 18.0));
            if strip.contains_rect(cr) {
                let cresp = ui.interact(cr, Id::new(("tab-x", idx, i)), Sense::click());
                if cresp.hovered() {
                    pc.rect_filled(cr, 5.0, Color32::from_black_alpha(14));
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
                    pos2(x0 + (x1 - x0) * t, rect.top() + TOP),
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

    let plus_x = if overflow { rect.right() - 46.0 } else { (x + 14.0).min(rect.right() - 18.0) };
    let pr = Rect::from_center_size(pos2(plus_x, cy), vec2(24.0, 24.0));
    let presp = ui.interact(pr, Id::new(("tab-plus", idx)), Sense::click());
    if presp.hovered() {
        p.rect_filled(pr, 6.0, HOVER);
    }
    icons::glyph(&p, pr.shrink(5.0), Glyph::Plus, TEXT_SOFT);
    if presp.on_hover_text(tr!("Новая вкладка (Ctrl+T)", "New tab (Ctrl+T)")).clicked() {
        out.actions.push(Action::NewTab);
    }

    // with many tabs: a drop-down listing all of them
    if overflow {
        let lr = Rect::from_center_size(pos2(rect.right() - 18.0, cy), vec2(24.0, 24.0));
        let lresp = ui.interact(lr, Id::new(("tab-list", idx)), Sense::click());
        if lresp.hovered() {
            p.rect_filled(lr, 6.0, HOVER);
        }
        icons::glyph(&p, lr.shrink(6.0), Glyph::ChevronDown, TEXT_SOFT);
        let popup = Id::new(("tab-list-popup", idx));
        if lresp.clone().on_hover_text(trf!("Все вкладки ({n})", "All tabs ({n})")).clicked() {
            ui.memory_mut(|m| m.toggle_popup(popup));
        }
        egui::popup::popup_below_widget(ui, popup, &lresp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            ui.set_min_width(260.0);
            egui::ScrollArea::vertical().max_height(popup_max_height(ui, lresp.rect, 420.0)).show(ui, |ui| {
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
    if tool_btn(ui, rr, Id::new(("tool", idx, 9)), Glyph::Refresh, true).on_hover_text(tr!("Обновить (Ctrl+R)", "Refresh (Ctrl+R)")).clicked() {
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
        segs.push((tr!("Этот компьютер", "This PC").into(), PathBuf::new()));
    }
    if let Some(c) = remote::crumbs(&panel.path) {
        segs.extend(c);
    } else if !panel.is_pc() {
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
    if dresp.on_hover_text(tr!("Ввести путь (Ctrl+L)", "Enter path (Ctrl+L)")).clicked() {
        out.actions.push(Action::EditPath);
    }
}

/// Common look for right-click menus: roomy spacing and soft hover rows.
fn menu_style(ui: &mut Ui, min_width: f32) {
    ui.set_min_width(min_width);
    let s = ui.style_mut();
    s.spacing.item_spacing = vec2(0.0, 0.0);
    // submenu buttons (egui's menu_button) line up with the icon column of menu rows
    s.spacing.button_padding = vec2(MENU_TEXT_X, 5.0);
    s.spacing.interact_size.y = MENU_ROW_H;
    let w = &mut s.visuals.widgets;
    for wv in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.rounding = Rounding::same(4.0);
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

/// Scrolls the menu body when it is taller than the window, so a long context menu
/// in a small window stays fully reachable instead of being cut off at the edge.
/// The menu is laid out plainly while it fits; the scroll area only appears once the
/// measured height (remembered from the previous frame) exceeds the window.
fn menu_scroll<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let max_h = (ui.ctx().screen_rect().height() - 24.0).max(120.0);
    let id = ui.id().with("menu-content-h");
    let prev: Option<f32> = ui.data(|d| d.get_temp(id));
    if prev.map_or(true, |h| h <= max_h) {
        let top = ui.cursor().top();
        let r = add_contents(ui);
        let h = ui.min_rect().bottom() - top;
        ui.data_mut(|d| d.insert_temp(id, h));
        r
    } else {
        let out = egui::ScrollArea::vertical().id_salt("menu-scroll").max_height(max_h).show(ui, add_contents);
        let h = out.content_size.y;
        ui.data_mut(|d| d.insert_temp(id, h));
        out.inner
    }
}

/// Max height for a drop-down opened below `anchor`: what fits under it in the window,
/// capped at `cap`, but never so small the list becomes useless.
fn popup_max_height(ui: &Ui, anchor: Rect, cap: f32) -> f32 {
    let below = ui.ctx().screen_rect().bottom() - anchor.bottom() - 24.0;
    below.min(cap).max(120.0)
}

/// Thin separator with extra breathing room between menu groups.
fn menu_sep(ui: &mut Ui) {
    ui.add_space(2.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment([pos2(r.left() + 10.0, r.center().y), pos2(r.right() - 10.0, r.center().y)], Stroke::new(1.0, BORDER));
    ui.add_space(2.0);
}

/// Small muted caption at the top of a menu.
fn menu_title(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(MENU_ICON_X);
        ui.label(RichText::new(text).color(MUTED).size(12.0));
    });
    ui.add_space(2.0);
}

/// Compact menu metrics, close to the native Windows context menu.
const MENU_ROW_H: f32 = 28.0;
const MENU_ICON_X: f32 = 12.0;
const MENU_TEXT_X: f32 = 38.0;
const MENU_PAD_R: f32 = 14.0;

/// Full-width menu row without an icon (the icon column stays reserved, as in Windows).
fn menu_item(ui: &mut Ui, text: &str, shortcut: &str, enabled: bool, danger: bool) -> Response {
    menu_item_icon(ui, None, text, shortcut, enabled, danger)
}

/// Full-width compact menu row: icon, label, shortcut on the right,
/// soft rounded highlight on hover. `danger` paints it red.
fn menu_item_icon(ui: &mut Ui, icon: Option<Glyph>, text: &str, shortcut: &str, enabled: bool, danger: bool) -> Response {
    let font = regular(13.5);
    let key_font = regular(12.0);
    let text_col = if !enabled { GRAY } else if danger { RED } else { TEXT };
    let key_col = if enabled { MUTED } else { Color32::from_rgb(200, 205, 214) };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, text_col);
    let key_galley = ui.painter().layout_no_wrap(shortcut.to_owned(), key_font, key_col);
    let gap = if shortcut.is_empty() { 0.0 } else { 28.0 };
    let want = MENU_TEXT_X + galley.size().x + gap + key_galley.size().x + MENU_PAD_R;
    let w = ui.available_width().max(want);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, MENU_ROW_H), sense);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let t = ui.ctx().animate_bool_with_time(resp.id, enabled && resp.hovered(), 0.08);
        if t > 0.0 {
            let base = if danger { Color32::from_rgb(254, 236, 236) } else { BLUE_SOFT };
            let fill = if resp.is_pointer_button_down_on() {
                if danger { Color32::from_rgb(252, 222, 222) } else { Color32::from_rgb(218, 232, 255) }
            } else {
                base
            };
            p.rect_filled(rect, 4.0, fill.gamma_multiply(t));
        }
        let col = if enabled && resp.hovered() && !danger { BLUE } else { text_col };
        if let Some(g) = icon {
            let ic = Rect::from_center_size(pos2(rect.left() + MENU_ICON_X + 8.0, rect.center().y), vec2(16.0, 16.0));
            let icol = if !enabled { GRAY } else if danger { RED } else if resp.hovered() { BLUE } else { TEXT_SOFT };
            icons::glyph(p, ic, g, icol);
        }
        let ty = rect.center().y - galley.size().y / 2.0;
        p.galley_with_override_text_color(pos2(rect.left() + MENU_TEXT_X, ty), galley, col);
        if !shortcut.is_empty() {
            let kx = rect.right() - MENU_PAD_R - key_galley.size().x;
            let ky = rect.center().y - key_galley.size().y / 2.0;
            p.galley(pos2(kx, ky), key_galley, key_col);
        }
    }
    resp
}

/// Right-click menu for the empty area of the file list.
fn empty_menu(resp: Response, panel: &mut Panel, out: &mut PaneOut) {
    let is_pc = panel.is_pc();
    resp.context_menu(|ui| {
        menu_style(ui, 260.0);
        menu_scroll(ui, |ui| empty_menu_items(ui, panel, out, is_pc));
    });
}

fn empty_menu_items(ui: &mut Ui, panel: &mut Panel, out: &mut PaneOut, is_pc: bool) {
    {
        let mut item = |ui: &mut Ui, g: Glyph, text: &str, key: &str, enabled: bool, a: Action| {
            let key = hotkeys::menu_key(&a, key);
            let r = menu_item_icon(ui, Some(g), text, &key, enabled, false);
            if r.clicked() {
                out.actions.push(a);
                ui.close_menu();
            }
        };
        item(ui, Glyph::FolderPlus, tr!("Новая папка", "New folder"), "F7", !is_pc, Action::MkDir);
        item(ui, Glyph::Document, tr!("Новый файл", "New file"), "F9", !is_pc, Action::NewFile);
        if cfg!(windows) {
            item(ui, Glyph::Plus, tr!("Создать", "New"), "›", !is_pc, Action::CreateWindows);
        }
        item(ui, Glyph::Clipboard, tr!("Вставить", "Paste"), "Ctrl+V", !is_pc, Action::ClipboardPaste);
        menu_sep(ui);
        item(ui, Glyph::Drive, tr!("Размеры всех папок", "Sizes of all folders"), "", !is_pc, Action::CalculateAllSizes);
        item(ui, Glyph::Close, tr!("Отменить расчёт размеров", "Cancel size calculation"), "", !panel.folder_sizes.pending.is_empty(), Action::CancelSizes);
        menu_sep(ui);
        item(ui, Glyph::Check, tr!("Выделить всё", "Select all"), "Ctrl+A", true, Action::MarkAll);
        item(ui, Glyph::Minus, tr!("Снять выделение", "Clear selection"), "Esc", !panel.marked.is_empty(), Action::ClearMarks);
        menu_sep(ui);
        ui.menu_button(RichText::new(tr!("Сортировка", "Sort")).color(TEXT), |ui| {
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
                let on = panel.sort == key;
                let arrow = if on { if panel.asc { "  ↑" } else { "  ↓" } } else { "" };
                let text = format!("{label}{arrow}");
                if menu_item_icon(ui, on.then_some(Glyph::Check), &text, "", true, false).clicked() {
                    panel.set_sort(key);
                    ui.close_menu();
                }
            }
        });
        item(ui, Glyph::Search, tr!("Найти файлы…", "Find files…"), "Alt+F7", true, Action::FindFiles);
        item(ui, Glyph::Refresh, tr!("Обновить", "Refresh"), "Ctrl+R", true, Action::Refresh);
        item(ui, Glyph::Eye, if panel.show_hidden { tr!("Не показывать скрытые", "Don’t show hidden") } else { tr!("Показывать скрытые", "Show hidden") }, "F11", true, Action::ToggleHidden);
        menu_sep(ui);
        item(ui, Glyph::Star, tr!("Добавить эту папку в избранное…", "Add this folder to favorites…"), "", true, Action::AddCurrentFavorite);
        item(ui, Glyph::Folder, tr!("Открыть в Проводнике", "Open in Explorer"), "", !is_pc, Action::OpenInExplorer);
        item(ui, Glyph::Pencil, tr!("Ввести путь", "Enter path"), "Ctrl+L", true, Action::EditPath);
        if remote::is_sftp(&panel.path) {
            item(ui, Glyph::Terminal, tr!("SSH-терминал", "SSH terminal"), "Ctrl+Shift+P", true, Action::OpenTerminal);
        }
        if cfg!(windows) {
            menu_sep(ui);
            item(ui, Glyph::Windows, tr!("Показать меню Windows", "Show Windows menu"), "", !is_pc, Action::FolderWindowsMenu);
            item(ui, Glyph::Info, tr!("Свойства папки", "Folder properties"), "", !is_pc, Action::FolderProperties);
        }
    }
}

/// Whether `path` is the drive (or device) `root` or lies on it.
fn on_drive(path: &Path, root: &Path) -> bool {
    if is_remote(root) {
        quokka_domain::remote::starts_with(path, root)
    } else {
        path.starts_with(root)
    }
}

/// What the Windows shell calls `path`: a phone's `mtp://…` item has a shell
/// parsing name, everything else is its own.
fn shell_name(path: &Path) -> PathBuf {
    if is_device(path) {
        if let Some(p) = quokka_infra::mtp::shell_path(path) {
            return p;
        }
    }
    path.to_path_buf()
}

/// Generic name of a drive without a volume label.
fn drive_kind_name(kind: DriveKind) -> &'static str {
    match kind {
        DriveKind::Removable => tr!("Съёмный диск", "USB Drive"),
        DriveKind::Network => tr!("Сетевой диск", "Network Drive"),
        DriveKind::Optical => tr!("Дисковод", "Optical Drive"),
        DriveKind::Ram => tr!("RAM-диск", "RAM Disk"),
        DriveKind::Device => tr!("Устройство", "Device"),
        DriveKind::Fixed | DriveKind::Unknown => tr!("Локальный диск", "Local Disk"),
    }
}

/// Colour of a drive usage bar: blue, orange from 85 %, red from 95 % (as Explorer).
fn usage_color(frac: f32) -> Color32 {
    if frac >= 0.95 { RED } else if frac >= 0.85 { ORANGE } else { BLUE }
}

/// Tooltip of a drive: name, a usage bar and used / free / total.
fn drive_tooltip(ui: &mut Ui, title: &str, info: Option<&DriveInfo>) {
    const W: f32 = 150.0;
    ui.set_width(W);
    ui.spacing_mut().item_spacing.y = 3.0;
    ui.add(egui::Label::new(RichText::new(title).font(semibold(13.0)).color(TEXT)).truncate());
    let Some((total, free)) = info.and_then(|i| i.space) else {
        let text = if info.is_some() { tr!("Устройство не готово", "Device not ready") } else { tr!("Чтение…", "Reading…") };
        ui.label(RichText::new(text).color(MUTED).size(12.0));
        return;
    };
    let used = total.saturating_sub(free);
    let frac = if total > 0 { used as f32 / total as f32 } else { 0.0 };
    // used (red) and free (blue): the amounts above, the bar, the total below
    ui.horizontal(|ui| {
        ui.label(RichText::new(format_size(used)).font(semibold(12.0)).color(RED)).on_hover_text(tr!("Занято", "Used"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format_size(free)).font(semibold(12.0)).color(BLUE)).on_hover_text(tr!("Свободно", "Free"));
        });
    });
    let (r, _) = ui.allocate_exact_size(vec2(W, 8.0), Sense::hover());
    let p = ui.painter();
    let round = r.height() / 2.0;
    p.rect_filled(r, round, BLUE);
    if frac > 0.0 {
        let split = r.left() + (r.width() * frac).max(round);
        let used_r = Rect::from_min_max(r.min, pos2(split.min(r.right()), r.bottom()));
        let rounding = if frac >= 0.999 { Rounding::same(round) } else { Rounding { nw: round, sw: round, ne: 0.0, se: 0.0 } };
        p.rect_filled(used_r, rounding, RED);
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(tr!("занято", "used")).color(MUTED).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(tr!("свободно", "free")).color(MUTED).size(11.0));
        });
    });
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(trf!("Всего {}", "Total {}", format_size(total))).color(TEXT_SOFT).size(12.0));
    });
}

/// Begin a rubber-band selection at screen y `press_y`.
fn start_band(panel: &mut Panel, press_y: f32, rows_r: Rect, keep: bool) {
    panel.selecting = true;
    panel.drag_base = if keep { panel.marked.clone() } else { Default::default() };
    panel.drag_start_y = (press_y - rows_r.top() + panel.scroll_offset).max(0.0);
}

#[derive(Clone, Copy)]
struct ColumnDrag {
    pointer_x: f32,
    width: f32,
    max_width: f32,
    direction: f32,
}

impl ColumnDrag {
    fn width_at(self, pointer_x: f32) -> f32 {
        (self.width + (self.pointer_x - pointer_x) * self.direction)
            .clamp(config::MIN_COL_WIDTH, self.max_width)
    }
}


/// Drag handles on the left edge of «Изменён», «Размер», «Тип»: dragging left widens
/// the column, «Имя» takes whatever is left. Double-click restores the default width.
fn resize_columns(ui: &mut Ui, idx: usize, head: Rect, widths: &mut [f32; config::COLUMN_COUNT - 1], layout: &config::ColumnLayout, out: &mut PaneOut) {
    let cols = Cols::layout(head, widths, layout);
    let mut edges = Vec::new();
    for id in 1..config::COLUMN_COUNT {
        if let Some((left, right)) = cols.cell(id) {
            let before_name = right <= cols.name.0;
            edges.push((id - 1, if before_name { right } else { left }, if before_name { -1.0 } else { 1.0 }));
        }
    }
    let name_w = cols.name.1 - cols.name.0;
    let mut hot = None;
    for (k, x, direction) in edges {
        let grip = Rect::from_min_max(pos2(x - 5.0, head.top()), pos2(x + 5.0, head.bottom()));
        let resp = ui.interact(grip, Id::new(("col-grip", idx, k)), Sense::click_and_drag());
        if resp.hovered() || resp.dragged() {
            hot = Some(k);
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let drag_id = Id::new(("column-drag", idx));
        if resp.drag_started() {
            let pointer_x = ui.input(|i| i.pointer.press_origin()).map_or(x, |p| p.x);
            // Widths of non-name columns are independent of the pane width.
            let width = cols.cell(k + 1).map(|c| c.1 - c.0).unwrap_or(widths[k]);
            let drag = ColumnDrag {
                pointer_x,
                width,
                max_width: (width + (name_w - MIN_NAME_W).max(0.0)).max(config::MIN_COL_WIDTH),
                direction,
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

fn column_label(id: usize) -> &'static str {
    match id {
        0 => tr!("Имя", "Name"),
        1 => tr!("Изменён", "Modified"),
        2 => tr!("Размер", "Size"),
        3 => tr!("Тип", "Type"),
        4 => tr!("Создан", "Created"),
        5 => tr!("Дата доступа", "Accessed"),
        6 => tr!("Расширение", "Extension"),
        _ => tr!("Атрибуты", "Attributes"),
    }
}

fn column_sort(id: usize) -> SortKey {
    [SortKey::Name, SortKey::Modified, SortKey::Size, SortKey::Type,
        SortKey::Created, SortKey::Accessed, SortKey::Extension, SortKey::Attributes][id]
}

fn header_menu(resp: Response, idx: usize, widths: &mut [f32; config::COLUMN_COUNT - 1], out: &mut PaneOut) {
    resp.context_menu(|ui| {
        menu_style(ui, 280.0);
        if menu_item_icon(ui, Some(Glyph::Columns), tr!("Настроить колонки…", "Configure columns…"), "", true, false).clicked() {
            out.actions.push(Action::ConfigureColumns(idx));
            ui.close_menu();
        }
        ui.separator();
        let changed = *widths != config::DEFAULT_COL_WIDTHS;
        if menu_item_icon(ui, None, tr!("Сбросить ширину колонок", "Reset column widths"), "", changed, false).clicked() {
            *widths = config::DEFAULT_COL_WIDTHS;
            out.cols_changed = true;
            ui.close_menu();
        }
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

fn table(ui: &mut Ui, panel: &mut Panel, active: bool, idx: usize, rect: Rect, col_widths: &mut [f32; config::COLUMN_COUNT - 1], layout: &mut config::ColumnLayout, out: &mut PaneOut, file_icons: Option<&mut crate::file_icons::FileIcons>) {
    let content_width = Cols::content_width(rect.width(), col_widths, layout);
    #[allow(deprecated)]
    ui.allocate_ui_at_rect(rect, |ui| {
        egui::ScrollArea::horizontal().id_salt(("columns-scroll", idx))
            .auto_shrink([false, false]).drag_to_scroll(false).show(ui, |ui| {
                ui.set_min_width(content_width);
                let (content, _) = ui.allocate_exact_size(vec2(content_width, ui.available_height()), Sense::hover());
                #[allow(deprecated)]
                ui.allocate_ui_at_rect(content, |ui| {
                    table_content(ui, panel, active, idx, content, col_widths, layout, out, file_icons);
                });
            });
    });
}

fn table_content(ui: &mut Ui, panel: &mut Panel, active: bool, idx: usize, rect: Rect, col_widths: &mut [f32; config::COLUMN_COUNT - 1], layout: &mut config::ColumnLayout, out: &mut PaneOut, mut file_icons: Option<&mut crate::file_icons::FileIcons>) {
    let opening = ui.ctx().data(|d| d.get_temp::<OpenFeedback>(open_feedback_id()));
    let now = ui.input(|i| i.time);
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), HEAD_H));
    resize_columns(ui, idx, head, col_widths, layout, out);
    let cols = Cols::layout(head, col_widths, layout);
    let p = ui.painter().clone();
    p.rect_filled(head, 8.0, HEADER_BG);

    let header_cols: Vec<_> = layout.order.into_iter().filter_map(|id| {
        let key = column_sort(id);
        cols.cell(id).map(|cell| (id, column_label(id), key, cell))
    }).collect();
    let grip_hot = |k: usize| ui.ctx().memory(|m| m.data.get_temp::<usize>(Id::new(("col-grip-hot", idx)))) == Some(k);
    for (k, (id, label, key, (x0, x1))) in header_cols.into_iter().enumerate() {
        // leave the column edges to the resize grips (see `resize_columns`)
        let cr = Rect::from_min_max(pos2(x0 + 5.0, head.top()), pos2(x1 - 5.0, head.bottom()));
        let resp = ui.interact(cr, Id::new(("head", idx, k)), Sense::click());
        if k > 0 {
            let col = id.checked_sub(1);
            let (h, c) = if col.is_some_and(grip_hot) { (13.0, BLUE) } else { (9.0, BORDER) };
            p.line_segment([pos2(x0, head.center().y - h), pos2(x0, head.center().y + h)], Stroke::new(if c == BLUE { 2.0 } else { 1.0 }, c));
        }
        let tx = if k == 0 { x0 + 11.0 } else { x0 + 12.0 };
        let hp = p.with_clip_rect(p.clip_rect().intersect(Rect::from_min_max(pos2(x0, head.top()), pos2(x1, head.bottom()))));
        let sorted = panel.sort == key;
        let available = (x1 - tx - 8.0 - if sorted { 18.0 } else { 0.0 }).max(0.0);
        let shown = elide_with_suffix(&hp, label, &regular(11.5), available, "...");
        hp.text(pos2(tx, head.center().y), Align2::LEFT_CENTER, &shown, regular(11.5), if resp.hovered() { BLUE } else { TEXT_SOFT });
        if shown != label { resp.clone().on_hover_text(label); }
        if sorted {
            sort_arrow(&hp, pos2(x1 - 12.0, head.center().y), panel.asc);
        }
        if resp.clicked() {
            panel.set_sort(key);
        }
        header_menu(resp, idx, col_widths, out);
    }

    let rows_r = Rect::from_min_max(pos2(rect.left(), head.bottom() + 4.0), rect.max);
    out.zone.folder = if panel.results.is_some() { None } else { Some(panel.path.clone()).filter(|p| !p.as_os_str().is_empty()) };
    let n = panel.visible.len();
    if n == 0 || panel.error.is_some() {
        let msg = match &panel.error {
            Some(e) => trf!("Не удалось прочитать папку: {e}", "Failed to read the folder: {e}"),
            None if panel.is_loading() => tr!("Загрузка…", "Loading…").into(),
            None if panel.filter_error.is_some() => panel.filter_error.clone().unwrap_or_default(),
            None if !panel.filter.is_empty() => tr!("Ничего не найдено", "Nothing found").into(),
            None => tr!("Папка пуста", "Folder is empty").into(),
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
                        out.zone.rows.push((rect.intersect(rows_r).intersect(ui.clip_rect()), e.path.clone()));
                    }
                    let pr = ui.painter();
                    let is_cur = i == panel.cursor;
                    let is_marked = panel.marked.contains(&e.path);
                    let bg = rect.shrink2(vec2(0.0, 1.0));
                    if is_cur {
                        pr.rect_filled(bg, 6.0, if active { SEL_ACTIVE } else { SEL_INACTIVE });
                    } else if is_marked {
                        pr.rect_filled(bg, 6.0, MARKED_BG);
                    } else if resp.hovered() {
                        pr.rect_filled(bg, 6.0, HOVER);
                    }
                    let open_age = opening.as_ref().filter(|o| o.path == e.path)
                        .map(|o| (now - o.started).max(0.0) as f32)
                        .filter(|age| *age < OPEN_FEEDBACK_SECS as f32);
                    let open_fade = open_age.map_or(0.0, |age| {
                        ((OPEN_FEEDBACK_SECS as f32 - age) / 0.3).clamp(0.0, 1.0)
                    });
                    if let Some(age) = open_age {
                        let pulse = (1.0 - age / OPEN_FEEDBACK_SECS as f32).powi(2);
                        pr.rect_filled(bg, 6.0, BLUE.gamma_multiply(0.16 * pulse));
                        let stripe = Rect::from_min_size(bg.min + vec2(2.0, 4.0), vec2(3.0, bg.height() - 8.0));
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
                        ((age / 0.32).clamp(0.0, 1.0) * std::f32::consts::PI).sin() * 2.0
                    });
                    let ir = Rect::from_center_size(pos2(cols.name.0 + 20.0, cy), vec2(18.0 + bounce, 18.0 + bounce));
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
                        let status = pr.with_clip_rect(pr.clip_rect().intersect(Rect::from_min_max(
                            pos2(cols.name.0, rect.top()), pos2(cols.name.1 - 8.0, rect.bottom()),
                        )));
                        let center = if show_open_status { pos2(cols.name.1 - 107.0, cy) } else { ir.center() };
                        let radius = if show_open_status { 4.0 } else { 12.0 };
                        let points = (0..=20).map(|step| {
                            let angle = age * 8.0 + step as f32 / 20.0 * std::f32::consts::PI * 1.5;
                            center + vec2(angle.cos(), angle.sin()) * radius
                        }).collect();
                        status.add(egui::Shape::line(points, Stroke::new(1.7, BLUE.gamma_multiply(open_fade))));
                        if show_open_status {
                            status.text(pos2(center.x + 11.0, cy), Align2::LEFT_CENTER,
                                tr!("Открывается…", "Opening…"), regular(11.0), BLUE.gamma_multiply(open_fade));
                        }
                    }
                    let name_color = if is_marked {
                        MARKED_FG
                    } else if e.hidden {
                        MUTED
                    } else {
                        TEXT
                    };
                    let label = if e.is_drive { trf!("Локальный диск ({})", "Local Disk ({})", e.name) } else { e.name.clone() };
                    let name_font = if is_marked { semibold(12.0) } else { regular(12.0) };
                    let name_x = cols.name.0 + 36.0;
                    let shown = elide_right(pr, &label, &name_font, name_clip.right() - name_x);
                    let elided = shown != label;
                    let text_r = pr.with_clip_rect(name_clip.intersect(pr.clip_rect())).text(
                        pos2(name_x, cy),
                        Align2::LEFT_CENTER,
                        shown,
                        name_font,
                        name_color,
                    );
                    if elided && ui.rect_contains_pointer(text_r) {
                        resp.clone().on_hover_text(label);
                    }
                    if resp.drag_started_by(egui::PointerButton::Primary) {
                        // by the icon or the name, or anywhere on a marked row (as in
                        // Explorer): drag the files; elsewhere: selection band
                        let grip = ir.union(text_r).expand2(vec2(6.0, 4.0)).intersect(name_clip);
                        let draggable = !e.is_parent && !e.is_drive && !panel.selecting;
                        let on_grip = press.is_some_and(|p| grip.contains(p));
                        if draggable && (on_grip || is_marked) {
                            let sources = if panel.marked.contains(&e.path) { panel.targets() } else { vec![e.path.clone()] };
                            panel.cursor = i;
                            out.drag_start = Some(sources);
                        } else {
                            start_band(panel, press.map_or(rect.center().y, |p| p.y), rows_r, ctrl);
                        }
                    }
                    if let Some((x0, x1)) = cols.modified {
                        let t = if e.is_parent || e.is_drive { String::new() } else { format_time(e.modified) };
                        let t = elide_right(pr, &t, &regular(11.5), x1 - x0 - 22.0);
                        pr.with_clip_rect(pr.clip_rect().intersect(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom()))))
                            .text(pos2(x0 + 12.0, cy), Align2::LEFT_CENTER, t, regular(11.5), TEXT_SOFT);
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
                    if let Some(size) = cols.size {
                        let size_txt = elide_right(pr, &size_txt, &regular(11.5), size.1 - size.0 - 24.0);
                        pr.text(pos2(size.1 - 16.0, cy), Align2::RIGHT_CENTER, size_txt, regular(11.5), TEXT_SOFT);
                        if e.is_dir && !e.is_parent && !e.is_drive {
                            let cell = Rect::from_min_max(pos2(size.0, rect.top()), pos2(size.1, rect.bottom()));
                            if let Some(size) = panel.folder_sizes.sizes.get(&e.path) {
                                let hint = if size.skipped > 0 {
                                    trf!("Неполный размер: {} байт. Пропущено недоступных объектов или ссылок: {}", "Incomplete size: {} bytes. Inaccessible items or links skipped: {}", size.bytes, size.skipped)
                                } else { trf!("{} байт, включая вложенные и скрытые файлы", "{} bytes, including nested and hidden files", size.bytes) };
                                if ui.rect_contains_pointer(cell) { resp.clone().on_hover_text(hint); }
                            }
                        }
                    }
                    for id in 4..config::COLUMN_COUNT {
                        if let Some((x0, x1)) = cols.cell(id) {
                            let text = if e.is_parent || e.is_drive { String::new() } else {
                                match id {
                                    4 => format_time(e.created),
                                    5 => format_time(e.accessed),
                                    6 => e.ext.clone(),
                                    _ => e.attributes.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "—".into()),
                                }
                            };
                            let text = elide_right(pr, &text, &regular(11.5), x1 - x0 - 22.0);
                            pr.with_clip_rect(pr.clip_rect().intersect(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom()))))
                                .text(pos2(x0 + 12.0, cy), Align2::LEFT_CENTER, text, regular(11.5), TEXT_SOFT);
                        }
                    }
                    if let Some((x0, x1)) = cols.kind {
                        let kind_txt = elide_right(pr, &e.type_name, &regular(11.5), x1 - x0 - 22.0);
                        pr.with_clip_rect(pr.clip_rect().intersect(Rect::from_min_max(pos2(x0, rect.top()), pos2(x1 - 4.0, rect.bottom()))))
                            .text(pos2(x0 + 12.0, cy), Align2::LEFT_CENTER, kind_txt, regular(11.5), TEXT_SOFT);
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
                            tr!("Добавить в избранное…\tCtrl+D", "Add to favorites…\tCtrl+D")
                        } else {
                            tr!("Добавить эту папку в избранное…\tCtrl+D", "Add this folder to favorites…\tCtrl+D")
                        };
                        let mut items: Vec<(&str, Glyph, Action)> = vec![
                            (tr!("Открыть\tEnter", "Open\tEnter"), Glyph::ArrowRight, Action::Open),
                            (tr!("Просмотр\tF3", "View\tF3"), Glyph::Eye, Action::View),
                            (tr!("Редактировать\tF4", "Edit\tF4"), Glyph::Pencil, Action::Edit),
                            (tr!("Встроенный редактор\tShift+F4", "Built-in editor\tShift+F4"), Glyph::Code, Action::EditBuiltin),
                            (tr!("Переименовать\tF2", "Rename\tF2"), Glyph::Pencil, Action::Rename),
                            (tr!("Групповое переименование…\tCtrl+M", "Multi-rename…\tCtrl+M"), Glyph::Columns, Action::MultiRename),
                            (tr!("Копировать\tCtrl+C", "Copy\tCtrl+C"), Glyph::Copy, Action::ClipboardCopy),
                            (tr!("Вырезать\tCtrl+X", "Cut\tCtrl+X"), Glyph::Scissors, Action::ClipboardCut),
                            (tr!("Вставить\tCtrl+V", "Paste\tCtrl+V"), Glyph::Clipboard, Action::ClipboardPaste),
                            (tr!("Копировать в другую панель\tF5", "Copy to other pane\tF5"), Glyph::Copy, Action::Copy),
                            (tr!("Переместить в другую панель\tF6", "Move to other pane\tF6"), Glyph::ArrowRight, Action::Move),
                            (tr!("Добавить в ZIP-архив…\tAlt+F5", "Add to ZIP archive…\tAlt+F5"), Glyph::Archive, Action::Zip),
                            (tr!("Конвертировать изображения…\tCtrl+Shift+E", "Convert images…\tCtrl+Shift+E"), Glyph::Image, Action::ConvertImages),
                            (fav, Glyph::Star, Action::AddFavorite),
                            (tr!("Удалить в корзину\tDel", "Move to Recycle Bin\tDel"), Glyph::Trash, Action::Delete),
                            (tr!("Удалить навсегда\tShift+Del", "Delete permanently\tShift+Del"), Glyph::Close, Action::DeletePermanent),
                            (tr!("Новая папка\tF7", "New folder\tF7"), Glyph::FolderPlus, Action::MkDir),
                            (tr!("Новый файл\tF9", "New file\tF9"), Glyph::Document, Action::NewFile),
                            (tr!("Отметить / снять\tSpace", "Mark / unmark\tSpace"), Glyph::Check, Action::Mark),
                            (tr!("Отметить всё\tCtrl+A", "Mark all\tCtrl+A"), Glyph::Check, Action::MarkAll),
                        ];
                        // the listing already knows the type: no disk access while the menu is open
                        if cfg!(windows) && !panel.is_pc() {
                            let index = items.iter().position(|(_, _, a)| *a == Action::NewFile).unwrap() + 1;
                            items.insert(index, (tr!("Создать\t›", "New\t›"), Glyph::Plus, Action::CreateWindows));
                        }
                        if !e.is_dir && e.ext == "zip" {
                            items.insert(10, (tr!("Распаковать…\tAlt+F6", "Extract…\tAlt+F6"), Glyph::Unpack, Action::Unzip));
                        }
                        if e.is_dir && !e.is_parent && !e.is_drive {
                            items.insert(1, (tr!("Рассчитать размер папок\tAlt+Enter", "Calculate folder size\tAlt+Enter"), Glyph::Drive, Action::CalculateSizes));
                        }
                        items.push((tr!("Размеры всех папок", "Sizes of all folders"), Glyph::Drive, Action::CalculateAllSizes));
                        if !panel.folder_sizes.pending.is_empty() {
                            items.push((tr!("Отменить расчёт размеров", "Cancel size calculation"), Glyph::Close, Action::CancelSizes));
                        }
                        if cfg!(windows) && !is_remote(&panel.path) && quokka_infra::archive::location(&panel.path).is_none() {
                            items.push((tr!("Показать меню Windows", "Show Windows menu"), Glyph::Windows, Action::WindowsMenu));
                        }
                        if cfg!(windows) {
                            items.push((tr!("Свойства	Ctrl+Alt+Enter", "Properties	Ctrl+Alt+Enter"), Glyph::Info, Action::Properties));
                        }
                        // a long menu in a small window scrolls instead of being cut off
                        menu_scroll(ui, |ui| for (label, g, a) in items {
                            if matches!(a, Action::Zip | Action::Delete | Action::MkDir | Action::Mark | Action::WindowsMenu)
                                || (a == Action::Properties && !cfg!(windows)) {
                                menu_sep(ui);
                            }
                            let (name, key) = label.split_once('\t').unwrap_or((label, ""));
                            let key = hotkeys::menu_key(&a, key);
                            let r = menu_item_icon(ui, Some(g), name, &key, true, matches!(a, Action::Delete | Action::DeletePermanent));
                            if r.clicked() {
                                out.actions.push(a);
                                ui.close_menu();
                            }
                        });
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
        let p = ui.painter().with_clip_rect(rows_r.intersect(ui.clip_rect()));
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
                trf!("Выбрано: {} ({})", "Selected: {} ({})", panel.marked.len(), format_size(panel.marked_size())),
                regular(14.0),
                MARKED_FG,
            )
            .right();
    }
    if panel.is_loading() {
        p.line_segment([pos2(x + 12.0, cy - 9.0), pos2(x + 12.0, cy + 9.0)], Stroke::new(1.0, BORDER));
        x = p.text(pos2(x + 24.0, cy), Align2::LEFT_CENTER, tr!("Загрузка…", "Loading…"), regular(14.0), MUTED).right();
    }
    let _ = x;
    let mut rx = rect.right() - 16.0;
    if !panel.is_pc() {
        let r = p.text(pos2(rx, cy), Align2::RIGHT_CENTER, trf!("Всего {}", "Total {}", format_size(panel.bytes)), regular(14.0), TEXT_SOFT);
        rx = r.left() - 12.0;
        if let Some((total, free)) = panel.disk {
            p.line_segment([pos2(rx, cy - 9.0), pos2(rx, cy + 9.0)], Stroke::new(1.0, BORDER));
            let used = 1.0 - free as f32 / total.max(1) as f32;
            let br = Rect::from_min_size(pos2(rx - 12.0 - 60.0, cy - 3.0), vec2(60.0, 6.0));
            icons::bar(&p, br, used, if used > 0.9 { RED } else { BLUE });
            p.text(pos2(br.left() - 8.0, cy), Align2::RIGHT_CENTER, trf!("Свободно {}", "{} free", format_size(free)), regular(14.0), TEXT_SOFT);
        }
    }
}

// ───────────────────────────── widgets ─────────────────────────────

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
        ui.label(RichText::new(tr!("В этом файле нет значков", "This file has no icons")).color(MUTED));
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
                if resp.on_hover_text(trf!("Значок №{i}", "Icon #{i}")).clicked() {
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
            ui.label(RichText::new(trf!("Страница {} из {} · всего значков: {total}", "Page {} of {} · icons in total: {total}", ed.page + 1, pages)).size(12.5).color(TEXT_SOFT));
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

// ───────────────────────────── JetBrains-style widgets ─────────────────────────────

/// Corner radius of flat (JetBrains-style) dialogs.
const FLAT_R: f32 = 10.0;
/// Background of side trees and footers in flat dialogs.
const NAV_BG: Color32 = Color32::from_rgb(247, 248, 250);
const NAV_SEL: Color32 = Color32::from_rgb(212, 226, 255);
const FIELD_BORDER: Color32 = Color32::from_rgb(201, 205, 214);
const SEARCH_HIT: Color32 = Color32::from_rgb(255, 232, 140);
const SETTINGS_NAV_W: f32 = 232.0;

/// Left-hand tree of settings pages (only the ones matching the search).
fn settings_nav(ui: &mut Ui, group: &mut SettingsGroup, visible: &[SettingsGroup]) {
    for (g, name, glyph) in SettingsGroup::all() {
        if !visible.contains(&g) {
            continue;
        }
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
        let on = *group == g;
        let p = ui.painter();
        if on {
            p.rect_filled(r, 5.0, NAV_SEL);
        } else if resp.hovered() {
            p.rect_filled(r, 5.0, TRACK);
        }
        icons::glyph(p, Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(16.0, 16.0)), glyph, if on { BLUE } else { SLATE });
        p.text(pos2(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, name, regular(13.5), TEXT);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, name));
        if resp.clicked() {
            *group = g;
        }
    }
}

/// Lay out `add` inside an area the caller has already allocated. Unlike `allocate_new_ui`
/// this never moves the parent's cursor, so an empty child can't pull later widgets up.
fn place<R>(ui: &mut Ui, builder: egui::UiBuilder, add: impl FnOnce(&mut Ui) -> R) -> R {
    let mut child = ui.new_child(builder);
    add(&mut child)
}

/// Titled separator: «Заголовок ─────────».
fn jb_group(ui: &mut Ui, title: &str) {
    ui.add_space(20.0);
    let g = ui.painter().layout_no_wrap(title.to_string(), semibold(13.0), TEXT);
    let size = g.size();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), size.y + 4.0), Sense::hover());
    let y = r.center().y;
    ui.painter().galley(pos2(r.left(), y - size.y / 2.0), g, TEXT);
    if r.left() + size.x + 10.0 < r.right() {
        ui.painter().hline((r.left() + size.x + 10.0)..=r.right(), y, Stroke::new(1.0, BORDER));
    }
    ui.add_space(8.0);
}

/// Grey explanatory text under a control, indented like JetBrains comments.
fn jb_comment(ui: &mut Ui, indent: f32, text: &str) {
    ui.horizontal_top(|ui| {
        ui.add_space(indent);
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(MUTED)).wrap());
    });
}

/// Checkbox with a comment under it; highlighted when it matches the settings search.
fn jb_option(ui: &mut Ui, value: &mut bool, label: &str, hint: &str, query: &str) -> bool {
    let hit = !query.is_empty() && (label.to_lowercase().contains(query) || hint.to_lowercase().contains(query));
    let bg = ui.painter().add(egui::Shape::Noop);
    let resp = ui.checkbox(value, RichText::new(label).size(13.5).color(TEXT));
    if hit {
        ui.painter().set(bg, egui::Shape::rect_filled(resp.rect.expand2(vec2(4.0, 1.0)), 4.0, SEARCH_HIT));
    }
    ui.spacing_mut().item_spacing.y = 2.0;
    jb_comment(ui, 26.0, hint);
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.add_space(10.0);
    resp.changed()
}

/// Compact dialog button: blue primary or bordered secondary.
fn jb_btn(ui: &mut Ui, text: &str, primary: bool) -> Response {
    let (fill, fg, stroke) = if primary {
        (BLUE, Color32::WHITE, Stroke::NONE)
    } else {
        (CARD, TEXT, Stroke::new(1.0, FIELD_BORDER))
    };
    sized_btn(ui, text, fill, fg, stroke)
}

/// The one size for text buttons, taken from «Готово» in the settings:
/// 13.5 pt text, 30 px high, at least 76 px wide, 16 × 4 padding, 5 px corners.
fn sized_btn(ui: &mut Ui, text: &str, fill: Color32, fg: Color32, stroke: Stroke) -> Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = vec2(16.0, 4.0);
        ui.add(egui::Button::new(RichText::new(text).size(13.5).color(fg)).fill(fill).stroke(stroke).rounding(5.0).min_size(vec2(76.0, 30.0)))
    })
    .inner
}

/// Small toolbar icon (list toolbars, field actions).
fn jb_tool(ui: &mut Ui, g: Glyph, enabled: bool, tip: &str) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.hovered() {
        ui.painter().rect_filled(r, 5.0, TRACK);
    }
    icons::glyph(ui.painter(), r.shrink(6.0), g, if enabled { TEXT_SOFT } else { Color32::from_rgb(200, 205, 214) });
    resp.on_hover_text(tip)
}

/// Option toggle that lives inside a search field («Aa», «W», «.*»).
fn field_toggle(ui: &mut Ui, label: &str, on: bool, tip: &str) -> Response {
    let font = if label == ".*" { mono(13.0) } else { semibold(12.0) };
    let col = if on { BLUE } else { TEXT_SOFT };
    let g = ui.painter().layout_no_wrap(label.to_string(), font, col);
    let (r, resp) = ui.allocate_exact_size(vec2((g.size().x + 10.0).max(24.0), 22.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect(r, 4.0, BLUE_SOFT, Stroke::new(1.0, BLUE));
    } else if resp.hovered() {
        p.rect_filled(r, 4.0, TRACK);
    }
    p.galley(r.center() - g.size() / 2.0, g, col);
    resp.on_hover_text(tip)
}

/// Bordered single-line field with a leading icon (a history menu when `history` isn't empty)
/// and trailing controls. `trailing` runs right-to-left: add the rightmost control first.
/// Returns the text edit output and whether a history entry was picked.
#[allow(clippy::too_many_arguments)]
fn jb_field(
    ui: &mut Ui,
    id: Id,
    text: &mut String,
    hint: &str,
    glyph: Glyph,
    font: egui::FontId,
    history: &[String],
    trailing: impl FnOnce(&mut Ui),
) -> (egui::text_edit::TextEditOutput, bool) {
    let focused = ui.memory(|m| m.has_focus(id));
    let mut picked = false;
    let enabled = ui.is_enabled();
    let out = egui::Frame::none()
        .fill(CARD)
        .stroke(if focused { Stroke::new(1.5, BLUE) } else { Stroke::new(1.0, FIELD_BORDER) })
        .rounding(5.0)
        .inner_margin(Margin { left: 8.0, right: 6.0, top: 3.0, bottom: 3.0 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let has_hist = !history.is_empty();
                let sense = if has_hist { Sense::click() } else { Sense::hover() };
                let (r, resp) = ui.allocate_exact_size(vec2(if has_hist { 30.0 } else { 22.0 }, 26.0), sense);
                if has_hist && resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, TRACK);
                }
                let gr = Rect::from_center_size(pos2(r.left() + 11.0, r.center().y), vec2(15.0, 15.0));
                icons::glyph(ui.painter(), gr, glyph, if focused { BLUE } else { SLATE });
                if has_hist {
                    let cr = Rect::from_center_size(pos2(r.right() - 6.0, r.center().y + 1.0), vec2(9.0, 9.0));
                    icons::glyph(ui.painter(), cr, Glyph::ChevronDown, SLATE);
                    let popup = id.with("history");
                    let resp = resp.on_hover_text(tr!("Недавние", "Recent"));
                    if resp.clicked() {
                        ui.memory_mut(|m| m.toggle_popup(popup));
                    }
                    egui::popup_below_widget(ui, popup, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                        menu_style(ui, 320.0);
                        menu_title(ui, tr!("Недавние", "Recent"));
                        egui::ScrollArea::vertical().max_height(popup_max_height(ui, resp.rect, 320.0)).show(ui, |ui| {
                            for h in history {
                                if menu_item(ui, h, "", true, false).clicked() {
                                    *text = h.clone();
                                    picked = true;
                                }
                            }
                        });
                    });
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    trailing(ui);
                    ui.add_space(4.0);
                    egui::TextEdit::singleline(text)
                        .id(id)
                        .frame(false)
                        .desired_width(ui.available_width())
                        .margin(vec2(4.0, 5.0))
                        .font(font)
                        .text_color(if enabled { TEXT } else { TEXT_SOFT })
                        .hint_text(RichText::new(hint).color(MUTED))
                        .show(ui)
                })
                .inner
            })
            .inner
        })
        .inner;
    (out, picked)
}

/// Joined toggle buttons; returns the clicked index.
fn segmented(ui: &mut Ui, items: &[&str], sel: usize) -> Option<usize> {
    let mut clicked = None;
    egui::Frame::none().stroke(Stroke::new(1.0, FIELD_BORDER)).rounding(5.0).inner_margin(Margin::same(2.0)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (i, it) in items.iter().enumerate() {
                let on = i == sel;
                let col = if on { BLUE } else { TEXT };
                let g = ui.painter().layout_no_wrap(it.to_string(), regular(13.0), col);
                let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 24.0), Sense::click());
                if on {
                    ui.painter().rect(r, 4.0, BLUE_SOFT, Stroke::new(1.0, Color32::from_rgb(160, 196, 250)));
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, TRACK);
                }
                ui.painter().galley(r.center() - g.size() / 2.0, g, col);
                if resp.clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

/// Shorten from the left («…\папка\файл») so the end of a path stays visible.
fn elide_left(p: &egui::Painter, s: &str, font: &egui::FontId, max_w: f32) -> String {
    let w = |t: &str| p.layout_no_wrap(t.to_string(), font.clone(), TEXT).size().x;
    if w(s) <= max_w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let t: String = std::iter::once('…').chain(chars[mid..].iter().copied()).collect();
        if w(&t) <= max_w {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    std::iter::once('…').chain(chars[lo..].iter().copied()).collect()
}

/// Shorten from the right («длинное назв…») so the text fits into `max_w`.
fn elide_right(p: &egui::Painter, s: &str, font: &egui::FontId, max_w: f32) -> String {
    elide_with_suffix(p, s, font, max_w, "…")
}

fn elide_with_suffix(p: &egui::Painter, s: &str, font: &egui::FontId, max_w: f32, suffix: &str) -> String {
    let w = |t: &str| p.layout_no_wrap(t.to_string(), font.clone(), TEXT).size().x;
    if s.is_empty() || w(s) <= max_w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    if w(suffix) > max_w { return String::new(); }
    let cut = |n: usize| -> String { chars[..n].iter().collect::<String>() + suffix };
    // longest prefix that still fits together with the ellipsis
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if w(&cut(mid)) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

fn kbd(ui: &mut Ui, k: &str) {
    let g = ui.painter().layout_no_wrap(k.to_string(), semibold(12.5), BLUE);
    let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 16.0, 24.0), Sense::hover());
    ui.painter().rect_filled(r, 6.0, BLUE_SOFT);
    ui.painter().galley(r.center() - g.size() / 2.0, g, BLUE);
}

fn btn(ui: &mut Ui, text: &str, fill: Color32, fg: Color32) -> Response {
    sized_btn(ui, text, fill, fg, Stroke::NONE)
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
    p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| tr!("Архив", "Archive").into())
}

fn kind_look(k: OpKind) -> (Glyph, Color32) {
    match k {
        OpKind::Copy => (Glyph::Copy, BLUE),
        OpKind::Move => (Glyph::ArrowRight, GREEN),
        OpKind::Delete => (Glyph::Trash, RED),
        OpKind::Archive => (Glyph::Archive, ORANGE),
        OpKind::Convert => (Glyph::Image, PURPLE),
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
    sized_btn(ui, text, INPUT_BG, fg, Stroke::NONE)
}

fn progress_text(t: &Task) -> String {
    let State::Running(job) = &t.state else { return String::new() };
    if job.total == 0 {
        tr!("Подготовка…", "Preparing…").into()
    } else if job.bytes {
        trf!("{} из {}", "{} of {}", format_size(job.done), format_size(job.total))
    } else {
        trf!("{} из {}", "{} of {}", job.done, job.total)
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
                State::Pending => (tr!("Ожидает", "Pending").to_string(), MUTED),
                State::Running(job) => {
                    let (br, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
                    icons::bar(ui.painter(), br, job.fraction(), color);
                    let cur = if job.current.is_empty() { String::new() } else { format!(" · {}", job.current) };
                    (format!("{}{cur}", progress_text(t)), TEXT_SOFT)
                }
                State::Done(m) => (m.clone(), GREEN),
                State::Failed(e) => (e.clone(), RED),
                State::Cancelled => (tr!("Отменено", "Cancelled").to_string(), MUTED),
            };
            ui.add(egui::Label::new(RichText::new(status).color(col).size(12.0)).truncate());
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            match &t.state {
                State::Running(_) => {
                    if icon_btn(ui, Glyph::Close, RED, tr!("Отменить", "Undo")).clicked() {
                        ops.push(QueueOp::Cancel(t.id));
                    }
                }
                State::Pending => {
                    if icon_btn(ui, Glyph::Close, TEXT_SOFT, tr!("Убрать из очереди", "Remove from queue")).clicked() {
                        ops.push(QueueOp::Cancel(t.id));
                    }
                    if !last_pending && icon_btn(ui, Glyph::ChevronDown, TEXT_SOFT, tr!("Позже", "Later")).clicked() {
                        ops.push(QueueOp::Down(t.id));
                    }
                    if !first_pending && icon_btn(ui, Glyph::ArrowUp, TEXT_SOFT, tr!("Раньше", "Earlier")).clicked() {
                        ops.push(QueueOp::Up(t.id));
                    }
                }
                State::Done(_) => {
                    if icon_btn(ui, Glyph::Close, MUTED, tr!("Убрать из списка", "Remove from list")).clicked() {
                        ops.push(QueueOp::Remove(t.id));
                    }
                }
                State::Failed(_) | State::Cancelled => {
                    if icon_btn(ui, Glyph::Close, MUTED, tr!("Убрать из списка", "Remove from list")).clicked() {
                        ops.push(QueueOp::Remove(t.id));
                    }
                    if icon_btn(ui, Glyph::Refresh, BLUE, tr!("Повторить", "Retry")).clicked() {
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
            ui.label(RichText::new(tr!("Очередь операций", "Operation queue")).font(semibold(16.0)).color(TEXT));
            let sub = match (q.running().is_some(), pending, q.paused) {
                (false, 0, _) => tr!("Нет активных операций", "No active operations").to_string(),
                (_, n, true) => trf!("Пауза · ожидает: {n}", "Paused · pending: {n}"),
                (true, 0, _) => tr!("Выполняется 1 операция", "1 operation running").to_string(),
                (_, n, _) => trf!("Ожидает: {n}", "Pending: {n}"),
            };
            ui.label(RichText::new(sub).size(12.5).color(MUTED));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_btn(ui, Glyph::ChevronDown, TEXT_SOFT, tr!("Свернуть (Ctrl+J)", "Collapse (Ctrl+J)")).clicked() {
                ops.push(QueueOp::Close);
            }
            if q.paused || pending > 0 {
                let (label, fg) = if q.paused { (tr!("Продолжить", "Resume"), BLUE) } else { (tr!("Пауза", "Pause"), TEXT_SOFT) };
                if small_btn(ui, label, fg).on_hover_text(tr!("Текущая операция доработает, следующие не начнутся", "The current operation will finish, the next ones won’t start")).clicked() {
                    ops.push(QueueOp::Pause);
                }
            }
        });
    });
    ui.add_space(8.0);
    if q.tasks.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new(tr!("Копирование, перемещение, удаление и архивы попадают сюда и выполняются по очереди.", "Copying, moving, deleting and archives go here and run one after another.")).size(12.5).color(MUTED));
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
            if small_btn(ui, tr!("Очистить завершённые", "Clear finished"), TEXT_SOFT).clicked() {
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
            (_, true) => tr!("Очередь на паузе", "Queue is paused").to_string(),
            (0, _) => String::new(),
            (n, _) => trf!("Ещё в очереди: {n}", "Still in queue: {n}"),
        };
        ui.label(RichText::new(note).size(12.0).color(MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if small_btn(ui, tr!("Вся очередь", "Whole queue"), BLUE).on_hover_text("Ctrl+J").clicked() {
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
    dialog_with(ctx, id, width, 22.0, 16.0, true, add)
}

/// JetBrains-style dialog: no padding, so side trees, lists and the footer run edge to edge.
fn flat_dialog<R>(ctx: &egui::Context, id: &str, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    dialog_with(ctx, id, width, 0.0, FLAT_R, true, add)
}

/// Like [`flat_dialog`], but without the corner close button: for dialogs that put
/// their own close button into their title bar.
fn flat_dialog_own_close<R>(ctx: &egui::Context, id: &str, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    dialog_with(ctx, id, width, 0.0, FLAT_R, false, add)
}

fn dialog_with<R>(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    margin: f32,
    rounding: f32,
    corner_close: bool,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
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
                .rounding(rounding)
                .stroke(Stroke::new(1.0, BORDER))
                .shadow(ctx.style().visuals.window_shadow)
                .inner_margin(Margin::same(margin))
                .show(ui, |ui| {
                    ui.set_width(width);
                    let out = add(ui);
                    if !corner_close {
                        return out;
                    }
                    // close button in the top-right corner, added last so it wins hit-testing over content
                    let frame = ui.min_rect().expand(margin);
                    let r = Rect::from_min_size(pos2(frame.right() - 38.0, frame.top() + 10.0), vec2(28.0, 28.0));
                    let resp = ui.interact(r, Id::new((id, "dialog-close")), Sense::click()).on_hover_text(tr!("Закрыть (Esc)", "Close (Esc)"));
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 6.0, TRACK);
                    }
                    icons::glyph(ui.painter(), r.shrink(8.0), Glyph::Close, if resp.hovered() { TEXT } else { TEXT_SOFT });
                    if resp.clicked() {
                        ctx.data_mut(|d| d.insert_temp(Id::new(DIALOG_CLOSE), true));
                        ctx.request_repaint();
                    }
                    out
                })
                .inner
        })
        .inner
}

const DIALOG_CLOSE: &str = "dialog-close-requested";

/// True once after the user clicked a dialog's close button; it then behaves like Esc.
fn take_dialog_close(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(Id::new(DIALOG_CLOSE))).unwrap_or(false)
}

// ───────────────────────────── viewer / editor loading ─────────────────────────────

fn load_viewer(path: PathBuf) -> Modal {
    Modal::Preview(Box::new(preview::Preview::new(path)))
}

fn load_editor(path: PathBuf) -> Modal {
    const LIMIT: u64 = 4 * 1024 * 1024;
    let too_big = || Modal::Message { title: tr!("Файл слишком большой", "File is too large").into(), text: tr!("Встроенный редактор открывает файлы до 4 МБ.", "The built-in editor opens files up to 4 MB.").into() };
    let read = if is_remote(&path) {
        match quokka_infra::xfer::read_limited(&path, LIMIT as usize + 1) {
            Ok((_, total)) if total > LIMIT => return too_big(),
            r => r.map(|(data, _)| data),
        }
    } else {
        if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > LIMIT {
            return too_big();
        }
        fs::read(&path)
    };
    match read {
        Ok(bytes) => {
            if bytes.iter().take(8192).any(|&b| b == 0) {
                return Modal::Message { title: tr!("Двоичный файл", "Binary file").into(), text: tr!("Это не текстовый файл — откройте его на просмотр (F3).", "This is not a text file — open it for viewing (F3).").into() };
            }
            match String::from_utf8(bytes) {
                Ok(text) => Modal::Editor { path, original: text.clone(), text, confirm_close: false },
                Err(_) => Modal::Message {
                    title: tr!("Неподдерживаемая кодировка", "Unsupported encoding").into(),
                    text: tr!("Редактировать можно только текст в UTF-8; для просмотра нажмите F3.", "Only UTF-8 text can be edited; press F3 to view.").into(),
                },
            }
        }
        Err(e) => Modal::Message { title: tr!("Не удалось открыть файл", "Failed to open the file").into(), text: e.to_string() },
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
        assert_eq!(clipboard_actions(&mut events, Modifiers::COMMAND), vec![Action::ClipboardCopy, Action::ClipboardCut, Action::ClipboardPaste]);
        assert!(matches!(events.as_slice(), [egui::Event::Text(_)]));
        assert!(clipboard_actions(&mut events, Modifiers::NONE).is_empty());
    }

    #[test]
    fn shift_delete_cut_event_means_permanent_delete() {
        // egui-winit on Windows reports Shift+Del as `Event::Cut` with no key event
        let mut events = vec![egui::Event::Cut];
        assert_eq!(clipboard_actions(&mut events, Modifiers::SHIFT), vec![Action::DeletePermanent]);
        assert!(events.is_empty());
        // Ctrl+Shift+X is still a cut
        let mut events = vec![egui::Event::Cut];
        assert_eq!(clipboard_actions(&mut events, Modifiers::COMMAND | Modifiers::SHIFT), vec![Action::ClipboardCut]);
    }

    #[test]
    fn one_ctrl_v_press_pastes_once() {
        use std::time::{Duration, Instant};
        let key = |pressed| egui::Event::Key { key: Key::V, physical_key: None, pressed,
            repeat: false, modifiers: Modifiers::COMMAND };
        let pastes = |ev: &[egui::Event]| ev.iter().filter(|e| matches!(e,
            egui::Event::Paste(_) | egui::Event::Key { pressed: true, .. })).count();
        let t = Instant::now();
        let mut latch = None;
        // Polled key state sees the press first: one injected paste.
        let mut ev = vec![];
        dedupe_paste(&mut ev, true, false, true, &mut latch, t);
        assert_eq!(pastes(&ev), 1);
        // The window message for the same press arrives a frame later: dropped.
        let mut ev = vec![key(true)];
        dedupe_paste(&mut ev, true, false, true, &mut latch, t + Duration::from_millis(16));
        assert_eq!(pastes(&ev), 0);
        // Key state already up, key-up still pending: nothing.
        let mut ev = vec![];
        dedupe_paste(&mut ev, false, false, true, &mut latch, t + Duration::from_millis(32));
        assert_eq!(pastes(&ev), 0);
        // Key-up arrives: no second paste, latch released.
        let mut ev = vec![key(false)];
        dedupe_paste(&mut ev, false, false, true, &mut latch, t + Duration::from_millis(48));
        assert_eq!(pastes(&ev), 0);
        assert!(latch.is_none());
        // The next press pastes again.
        let mut ev = vec![key(true)];
        dedupe_paste(&mut ev, true, false, true, &mut latch, t + Duration::from_millis(500));
        assert_eq!(pastes(&ev), 1);
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut finished = Vec::new();
        while queue.is_active() {
            finished.extend(queue.poll(&StdFileOps, &Waker::noop()));
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
    fn quick_repeated_double_clicks_each_open() {
        // «..» twice in a row: egui reports the 4th click as a triple click.
        let mut h = Harness::default();
        h.frame(vec![], Modifiers::NONE);
        assert!(h.primary(25.0).is_empty());
        assert_eq!(h.primary(25.0), vec![0]);
        assert!(h.primary(25.0).is_empty());
        assert_eq!(h.primary(25.0), vec![0]);
    }

    #[test]
    fn double_click_within_one_slow_frame_opens() {
        let mut h = Harness::default();
        h.frame(vec![], Modifiers::NONE);
        let (pos, jitter) = (pos2(30.0, 25.0), pos2(33.0, 27.0));
        let button = |pos, pressed| egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE,
        };
        let opened = h.frame(vec![
            egui::Event::PointerMoved(pos), button(pos, true), button(pos, false),
            egui::Event::PointerMoved(jitter), button(jitter, true), button(jitter, false),
        ], Modifiers::NONE);
        assert_eq!(opened, vec![0]);
    }

    #[test]
    fn clicks_on_far_apart_spots_of_one_row_do_not_open() {
        let mut h = Harness::default();
        h.frame(vec![], Modifiers::NONE);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE,
        };
        for x in [30.0, 150.0] {
            let pos = pos2(x, 25.0);
            assert!(h.frame(vec![egui::Event::PointerMoved(pos), button(pos, true)], Modifiers::NONE).is_empty());
            assert!(h.frame(vec![button(pos, false)], Modifiers::NONE).is_empty());
        }
        // Same, with both clicks delivered in one slow frame.
        h.time += 1.0;
        let (a, b) = (pos2(30.0, 25.0), pos2(150.0, 25.0));
        assert!(h.frame(vec![
            egui::Event::PointerMoved(a), button(a, true), button(a, false),
            egui::Event::PointerMoved(b), button(b, true), button(b, false),
        ], Modifiers::NONE).is_empty());
    }

    #[test]
    fn jittery_double_click_opens() {
        let mut h = Harness::default();
        h.frame(vec![], Modifiers::NONE);
        assert!(h.primary(25.0).is_empty());
        assert_eq!(h.primary(29.0), vec![0]);
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
    fn layout_moves_name_and_hides_optional_columns() {
        let rect = Rect::from_min_size(pos2(10.0, 0.0), vec2(800.0, 30.0));
        let layout = config::ColumnLayout { order: [3, 2, 0, 1, 4, 5, 6, 7], visible: [true, true, false, true, false, false, false, false] };
        let cols = Cols::layout(rect, &config::DEFAULT_COL_WIDTHS, &layout);
        assert_eq!(cols.kind, Some((10.0, 160.0)));
        assert_eq!(cols.name, (160.0, 640.0));
        assert_eq!(cols.modified, Some((640.0, 810.0)));
        assert!(cols.size.is_none());
    }

    #[test]
    fn name_uses_all_space_when_optional_columns_are_hidden() {
        let rect = Rect::from_min_size(pos2(10.0, 0.0), vec2(200.0, 30.0));
        let layout = config::ColumnLayout { order: [1, 3, 2, 0, 4, 5, 6, 7], visible: [true, false, false, false, false, false, false, false] };
        let cols = Cols::layout(rect, &config::DEFAULT_COL_WIDTHS, &layout);
        assert_eq!(cols.name, (10.0, 210.0));
        assert!(cols.modified.is_none() && cols.size.is_none() && cols.kind.is_none());
    }

    #[test]
    fn resizing_only_changes_name_width_and_keeps_other_columns() {
        let layout = config::ColumnLayout { order: [2, 3, 1, 0, 4, 5, 6, 7], visible: [true; config::COLUMN_COUNT] };
        let widths = config::DEFAULT_COL_WIDTHS;
        let used: f32 = widths.iter().sum();
        for width in [100.0, 500.0, 1400.0, 1800.0] {
            let content_width = Cols::content_width(width, &widths, &layout);
            let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(content_width, 30.0));
            let cols = Cols::layout(rect, &widths, &layout);
            assert_eq!(cols.name.1 - cols.name.0, (width - used).max(MIN_NAME_W));
            for id in 1..config::COLUMN_COUNT {
                let (left, right) = cols.cell(id).expect("enabled column must never disappear");
                assert_eq!(right - left, widths[id - 1]);
            }
            let cells: Vec<_> = layout.order.into_iter().filter_map(|id| cols.cell(id)).collect();
            assert_eq!(cells.first().unwrap().0, 0.0);
            assert_eq!(cells.last().unwrap().1, content_width);
            for pair in cells.windows(2) { assert_eq!(pair[0].1, pair[1].0); }
        }
    }

    #[test]
    fn overflowing_columns_stay_within_table_viewport() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let services = Services { fs: Arc::new(quokka_infra::stdfs::StdFs), watcher: None, waker: Waker::noop() };
        let mut panel = Panel::new(std::env::current_dir().unwrap(), false, services);
        panel.wait_loaded();
        let mut widths = config::DEFAULT_COL_WIDTHS;
        let mut layout = config::ColumnLayout { visible: [true; config::COLUMN_COUNT], ..Default::default() };
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(600.0, 500.0))),
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let viewport = Rect::from_min_size(ui.cursor().min, vec2(400.0, 300.0));
                    #[allow(deprecated)]
                    ui.allocate_ui_at_rect(viewport, |ui| {
                        let mut out = PaneOut::default();
                        table(ui, &mut panel, true, 0, viewport, &mut widths, &mut layout, &mut out, None);
                        assert!(ui.min_rect().height() <= viewport.height() + 1.0,
                            "ui {:?}, viewport {:?}", ui.min_rect(), viewport);
                        assert!(panel.view_height <= viewport.height() - HEAD_H);
                    });
                    assert_eq!(widths, config::DEFAULT_COL_WIDTHS);
                    assert!(layout.visible.iter().all(|v| *v));
                });
            });
        }
    }

    #[test]
    fn header_ellipsis_fits_width_including_sort_arrow_reservation() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let p = ctx.debug_painter();
            let font = regular(11.5);
            for text in ["Дата создания", "Extension", "Имя"] {
                for width in [0.0, 12.0, 35.0, 60.0, 300.0] {
                    let shown = elide_with_suffix(&p, text, &font, width, "...");
                    let measured = p.layout_no_wrap(shown.clone(), font.clone(), TEXT).size().x;
                    assert!(measured <= width);
                    assert!(shown == text || shown.is_empty() || shown.ends_with("..."));
                }
            }
        });
    }

    #[test]
    fn columns_before_name_resize_from_the_right_edge() {
        let drag = ColumnDrag { pointer_x: 100.0, width: 100.0, max_width: 200.0,
            direction: -1.0 };
        assert_eq!(drag.width_at(140.0), 140.0);
        assert_eq!(drag.width_at(20.0), config::MIN_COL_WIDTH);
    }

    #[test]
    fn resize_preserves_small_movements_and_recovers_from_limits() {
        let drag = ColumnDrag { pointer_x: 500.0, width: 100.0, max_width: 200.0, direction: 1.0 };
        assert!((drag.width_at(499.8) - 100.2).abs() < 0.001);
        assert_eq!(drag.width_at(900.0), config::MIN_COL_WIDTH);
        assert_eq!(drag.width_at(0.0), 200.0);
        assert_eq!(drag.width_at(500.0), 100.0);
    }

    #[test]
    fn only_explicitly_disabled_columns_are_hidden() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(580.0, 30.0));
        let mut layout = config::ColumnLayout::default();
        assert!(Cols::layout(rect, &config::DEFAULT_COL_WIDTHS, &layout).kind.is_some());
        layout.visible[3] = false;
        let after = Cols::layout(rect, &config::DEFAULT_COL_WIDTHS, &layout);
        assert!(after.kind.is_none());
        assert_eq!(after.modified.unwrap().0, 310.0);
    }

    #[test]
    fn column_dragged_to_the_limit_stays_visible() {
        // Rounding and pane width never alter the enabled column set.
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 30.0));
        let cols = Cols::new(rect, &[170.0, 100.0, 150.0004, 170.0, 170.0, 100.0, 110.0]);
        assert!(cols.kind.is_some() && cols.modified.is_some());
    }
}
