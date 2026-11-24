//! Keyboard shortcuts the user can reassign (Settings → Keyboard shortcuts).
//!
//! Each command has a stable id (saved in the settings file), its default keys and
//! the keys the user gave it. A combination belongs to at most one command: when it
//! is given to another one, it is taken away from the first after the user agrees.
//! Some keys are fixed (list navigation, Enter, Esc, Tab, the clipboard) and can't
//! be reassigned.

use super::Action;
use eframe::egui::{self, Key, Modifiers};
use quokka_domain::tr;
use std::collections::BTreeMap;
use std::sync::RwLock;

/// One key with modifiers. `ctrl` is the Command key on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Combo {
    pub const fn new(ctrl: bool, alt: bool, shift: bool, key: Key) -> Combo {
        Combo { ctrl, alt, shift, key }
    }

    pub fn from_event(key: Key, m: Modifiers) -> Combo {
        Combo { ctrl: m.command || m.ctrl, alt: m.alt, shift: m.shift, key }
    }

    /// "Ctrl+Shift+E", "Alt+F7", "Del" …
    pub fn label(&self) -> String {
        let mut s = String::new();
        for (on, name) in [(self.ctrl, "Ctrl+"), (self.alt, "Alt+"), (self.shift, "Shift+")] {
            if on {
                s.push_str(name);
            }
        }
        s.push_str(key_label(self.key));
        s
    }

    /// Parse what [`Combo::to_config`] wrote ("Ctrl+Shift+E", "Alt+ArrowLeft").
    pub fn parse(text: &str) -> Option<Combo> {
        let mut c = Combo::new(false, false, false, Key::A);
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" => c.ctrl = true,
                "alt" => c.alt = true,
                "shift" => c.shift = true,
                // the key is saved by name ("Plus"), so an empty part is a broken entry
                "" => return None,
                _ => key = Some(Key::from_name(part).or_else(|| key_from_label(part))?),
            }
        }
        c.key = key?;
        Some(c)
    }

    pub fn to_config(&self) -> String {
        let mut s = String::new();
        for (on, name) in [(self.ctrl, "Ctrl+"), (self.alt, "Alt+"), (self.shift, "Shift+")] {
            if on {
                s.push_str(name);
            }
        }
        s.push_str(self.key.name());
        s
    }

    fn is_function_key(&self) -> bool {
        matches!(self.key, Key::F1 | Key::F2 | Key::F3 | Key::F4 | Key::F5 | Key::F6 | Key::F7 | Key::F8 | Key::F9 | Key::F10 | Key::F11 | Key::F12
            | Key::F13 | Key::F14 | Key::F15 | Key::F16 | Key::F17 | Key::F18 | Key::F19 | Key::F20)
    }

    /// Whether the combination would get in the way of typing in a text field
    /// (the path box): plain and Shift+ keys other than F-keys, and Ctrl+A.
    pub fn types_text(&self) -> bool {
        (!self.ctrl && !self.alt && !self.is_function_key()) || (self.ctrl && !self.alt && !self.shift && self.key == Key::A)
    }
}

fn key_label(key: Key) -> &'static str {
    match key {
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::Delete => "Del",
        Key::Escape => "Esc",
        Key::PageUp => "PgUp",
        Key::PageDown => "PgDn",
        Key::Insert => "Ins",
        Key::Backspace => "Backspace",
        k => k.name(),
    }
}

fn key_from_label(s: &str) -> Option<Key> {
    Some(match s {
        "←" => Key::ArrowLeft,
        "→" => Key::ArrowRight,
        "↑" => Key::ArrowUp,
        "↓" => Key::ArrowDown,
        "Del" => Key::Delete,
        "Esc" => Key::Escape,
        "PgUp" => Key::PageUp,
        "PgDn" => Key::PageDown,
        "Ins" => Key::Insert,
        _ => return None,
    })
}

const fn k(key: Key) -> Combo {
    Combo::new(false, false, false, key)
}
const fn ctrl(key: Key) -> Combo {
    Combo::new(true, false, false, key)
}
const fn alt(key: Key) -> Combo {
    Combo::new(false, true, false, key)
}
const fn shift(key: Key) -> Combo {
    Combo::new(false, false, true, key)
}
const fn ctrl_shift(key: Key) -> Combo {
    Combo::new(true, false, true, key)
}
const fn ctrl_alt(key: Key) -> Combo {
    Combo::new(true, true, false, key)
}

/// Section of the shortcuts list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Files,
    Selection,
    Navigation,
    View,
    Tools,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Files => tr!("Файлы", "Files"),
            Group::Selection => tr!("Выделение", "Selection"),
            Group::Navigation => tr!("Навигация и вкладки", "Navigation and tabs"),
            Group::View => tr!("Вид", "View"),
            Group::Tools => tr!("Инструменты", "Tools"),
        }
    }
}

/// A command that can have keys.
pub struct Command {
    /// Saved in the settings file; never change an existing id.
    pub id: &'static str,
    pub action: Action,
    pub group: Group,
    pub name: &'static str,
    pub defaults: Vec<Combo>,
}

pub fn commands() -> Vec<Command> {
    use Group::*;
    let c = |id, action, group, name, defaults: &[Combo]| Command { id, action, group, name, defaults: defaults.to_vec() };
    vec![
        c("view", Action::View, Files, tr!("Просмотр", "View"), &[k(Key::F3)]),
        c("edit", Action::Edit, Files, tr!("Правка во внешнем редакторе", "Edit in external editor"), &[k(Key::F4)]),
        c("edit_builtin", Action::EditBuiltin, Files, tr!("Правка во встроенном редакторе", "Edit in built-in editor"), &[shift(Key::F4)]),
        c("rename", Action::Rename, Files, tr!("Переименовать", "Rename"), &[k(Key::F2)]),
        c("multi_rename", Action::MultiRename, Files, tr!("Групповое переименование", "Multi-rename"), &[ctrl(Key::M)]),
        c("copy", Action::Copy, Files, tr!("Копировать в другую панель", "Copy to other pane"), &[k(Key::F5)]),
        c("move", Action::Move, Files, tr!("Переместить в другую панель", "Move to other pane"), &[k(Key::F6)]),
        c("mkdir", Action::MkDir, Files, tr!("Новая папка", "New folder"), &[k(Key::F7)]),
        c("new_file", Action::NewFile, Files, tr!("Новый файл", "New file"), &[k(Key::F9)]),
        c("delete", Action::Delete, Files, tr!("Удалить в корзину", "Move to Recycle Bin"), &[k(Key::F8), k(Key::Delete)]),
        c("delete_permanent", Action::DeletePermanent, Files, tr!("Удалить навсегда", "Delete permanently"), &[shift(Key::F8), shift(Key::Delete)]),
        c("undo", Action::UndoDelete, Files, tr!("Отменить удаление / переименование", "Undo delete / rename"), &[ctrl(Key::Z)]),
        c("zip", Action::Zip, Files, tr!("Создать ZIP-архив", "Create ZIP archive"), &[alt(Key::F5)]),
        c("unzip", Action::Unzip, Files, tr!("Распаковать ZIP", "Extract ZIP"), &[alt(Key::F6)]),
        c("convert", Action::ConvertImages, Files, tr!("Конвертировать изображения", "Convert images"), &[ctrl_shift(Key::E)]),
        c("sizes", Action::CalculateSizes, Files, tr!("Размер выбранных папок", "Size of selected folders"), &[alt(Key::Enter)]),
        c("properties", Action::Properties, Files, tr!("Свойства", "Properties"), &[ctrl_alt(Key::Enter)]),
        c("windows_menu", Action::WindowsMenu, Files, tr!("Показать меню Windows", "Show Windows menu"), &[shift(Key::F10)]),
        c("mark_all", Action::MarkAll, Selection, tr!("Выделить всё", "Select all"), &[ctrl(Key::A)]),
        c("select_mask", Action::SelectMask, Selection, tr!("Выделить по маске", "Select by mask"), &[ctrl(Key::S)]),
        c("unselect_mask", Action::UnselectMask, Selection, tr!("Снять выделение по маске", "Deselect by mask"), &[ctrl_shift(Key::S)]),
        c("invert_selection", Action::InvertSelection, Selection, tr!("Инвертировать выделение", "Invert selection"), &[ctrl_shift(Key::I)]),
        c("back", Action::Back, Navigation, tr!("Назад", "Back"), &[alt(Key::ArrowLeft)]),
        c("forward", Action::Forward, Navigation, tr!("Вперёд", "Forward"), &[alt(Key::ArrowRight)]),
        c("go_up", Action::GoUp, Navigation, tr!("Вверх на уровень", "Up one level"), &[alt(Key::ArrowUp)]),
        c("edit_path", Action::EditPath, Navigation, tr!("Ввести путь", "Enter path"), &[ctrl(Key::L)]),
        c("new_tab", Action::NewTab, Navigation, tr!("Новая вкладка", "New tab"), &[ctrl(Key::T)]),
        c("close_tab", Action::CloseTab, Navigation, tr!("Закрыть вкладку", "Close tab"), &[ctrl(Key::W)]),
        c("add_favorite", Action::AddFavorite, Navigation, tr!("Добавить в избранное", "Add to favorites"), &[ctrl(Key::D)]),
        c("refresh", Action::Refresh, View, tr!("Обновить", "Refresh"), &[ctrl(Key::R)]),
        c("toggle_hidden", Action::ToggleHidden, View, tr!("Скрытые файлы", "Hidden files"), &[k(Key::F11)]),
        c("toggle_dual", Action::ToggleDual, View, tr!("Одна / две панели", "One / two panes"), &[k(Key::F12)]),
        c("quick_view", Action::QuickView, View, tr!("Быстрый просмотр", "Quick view"), &[ctrl(Key::Q)]),
        c("queue", Action::ToggleQueue, View, tr!("Очередь операций", "Operation queue"), &[ctrl(Key::J)]),
        c("focus_search", Action::FocusSearch, View, tr!("Поиск по имени", "Search by name"), &[ctrl(Key::F)]),
        c("toggle_regex", Action::ToggleRegex, View, tr!("Регулярные выражения в поиске", "Regular expressions in search"), &[alt(Key::R)]),
        c("find_files", Action::FindFiles, Tools, tr!("Найти файлы", "Find files"), &[alt(Key::F7)]),
        c("terminal", Action::OpenTerminal, Tools, tr!("Терминал в текущей папке", "Terminal in current folder"), &[ctrl_shift(Key::P)]),
        c("connect", Action::Connect, Tools, tr!("Подключение к серверу", "Connect to a server"), &[ctrl(Key::K)]),
        c("explorer", Action::OpenInExplorer, Tools, tr!("Открыть в Проводнике", "Open in Explorer"), &[]),
        c("settings", Action::Settings, Tools, tr!("Настройки", "Settings"), &[]),
        c("help", Action::Help, Tools, tr!("Справка", "Help"), &[k(Key::F1)]),
        c("quit", Action::Quit, Tools, tr!("Выход", "Exit"), &[k(Key::F10)]),
    ]
}

/// Keys that always do the same thing and cannot be given to a command.
pub fn reserved(c: &Combo) -> Option<&'static str> {
    let plain = !c.ctrl && !c.alt;
    let nav = matches!(c.key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::PageUp | Key::PageDown | Key::Home | Key::End);
    Some(match c.key {
        _ if plain && nav => tr!("перемещение по списку", "moving through the list"),
        Key::Enter if plain && !c.shift => tr!("открыть", "open"),
        Key::Escape if plain => tr!("отмена / снять выделение", "cancel / clear selection"),
        Key::Tab if plain => tr!("другая панель", "other pane"),
        Key::Space if plain && !c.shift => tr!("отметить", "mark"),
        Key::Insert if plain && !c.shift => tr!("отметить и вниз", "mark and move down"),
        Key::Backspace if plain && !c.shift => tr!("вверх на уровень", "up one level"),
        Key::C if c.ctrl && !c.alt && !c.shift => tr!("копировать в буфер", "copy to clipboard"),
        Key::X if c.ctrl && !c.alt && !c.shift => tr!("вырезать в буфер", "cut to clipboard"),
        Key::V if c.ctrl && !c.alt && !c.shift => tr!("вставить из буфера", "paste from clipboard"),
        _ => return None,
    })
}

/// Commands with their current keys.
pub struct Keymap {
    pub commands: Vec<Command>,
    /// Keys per command, same order as `commands`.
    pub keys: Vec<Vec<Combo>>,
}

impl Keymap {
    pub fn defaults() -> Keymap {
        let commands = commands();
        let keys = commands.iter().map(|c| c.defaults.clone()).collect();
        Keymap { commands, keys }
    }

    /// Defaults with the user's changes (`id → "Ctrl+X, F5"`, empty = no keys).
    /// A combination claimed by a changed command is dropped from the defaults of
    /// any other one, so a saved reassignment stays a reassignment.
    pub fn with_overrides(overrides: &BTreeMap<String, String>) -> Keymap {
        let mut map = Keymap::defaults();
        let mut claimed = Vec::new();
        for (i, cmd) in map.commands.iter().enumerate() {
            if let Some(text) = overrides.get(cmd.id) {
                let keys: Vec<Combo> = text.split(',').filter_map(Combo::parse).filter(|c| reserved(c).is_none()).collect();
                claimed.extend(keys.iter().map(|c| (*c, i)));
                map.keys[i] = keys;
            }
        }
        for (combo, owner) in claimed {
            for (i, keys) in map.keys.iter_mut().enumerate() {
                if i != owner {
                    keys.retain(|c| *c != combo);
                }
            }
        }
        map
    }

    /// What differs from the defaults, for the settings file.
    pub fn overrides(&self) -> BTreeMap<String, String> {
        self.commands
            .iter()
            .zip(&self.keys)
            .filter(|(c, k)| c.defaults != **k)
            .map(|(c, k)| (c.id.to_string(), k.iter().map(Combo::to_config).collect::<Vec<_>>().join(", ")))
            .collect()
    }

    pub fn index(&self, id: &str) -> Option<usize> {
        self.commands.iter().position(|c| c.id == id)
    }

    /// Command that has `combo` now.
    pub fn owner(&self, combo: &Combo) -> Option<usize> {
        self.keys.iter().position(|k| k.contains(combo))
    }

    pub fn action_for(&self, combo: &Combo) -> Option<Action> {
        self.owner(combo).map(|i| self.commands[i].action.clone())
    }

    /// Give `combo` to command `i`, taking it from any other command.
    pub fn assign(&mut self, i: usize, combo: Combo, replace: Option<usize>) {
        for keys in &mut self.keys {
            keys.retain(|c| *c != combo);
        }
        match replace.filter(|&r| r < self.keys[i].len()) {
            Some(r) => self.keys[i][r] = combo,
            None => self.keys[i].push(combo),
        }
    }

    pub fn shortcut(&self, a: &Action) -> String {
        self.commands
            .iter()
            .position(|c| c.action == *a)
            .map(|i| self.keys[i].iter().take(2).map(Combo::label).collect::<Vec<_>>().join(" / "))
            .unwrap_or_default()
    }

    pub fn is_default(&self) -> bool {
        self.commands.iter().zip(&self.keys).all(|(c, k)| c.defaults == *k)
    }
}

static CURRENT: RwLock<Option<Keymap>> = RwLock::new(None);

/// Make `map` the keymap the whole interface uses.
pub fn install(map: Keymap) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(map);
}

pub fn with<R>(f: impl FnOnce(&Keymap) -> R) -> R {
    let guard = CURRENT.read().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(map) => f(map),
        None => f(&Keymap::defaults()),
    }
}

/// Menu text for the shortcut of `a`: the current keys of its command, or
/// `fallback` for actions that are not reassignable (clipboard, Enter …).
pub fn menu_key(a: &Action, fallback: &str) -> String {
    with(|m| if m.commands.iter().any(|c| c.action == *a) { m.shortcut(a) } else { fallback.to_string() })
}

/// Take presses of the command `id` out of `events` (for dialogs that react to
/// one shortcut and leave every other key to their text fields).
pub fn take_command(events: &mut Vec<egui::Event>, id: &str) -> bool {
    with(|m| {
        let Some(i) = m.index(id) else { return false };
        let before = events.len();
        events.retain(|e| match e {
            egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } => ![Some(*key), *physical_key]
                .into_iter()
                .flatten()
                .any(|k| m.keys[i].contains(&Combo::from_event(k, *modifiers))),
            _ => true,
        });
        events.len() != before
    })
}

/// Take the key presses bound to commands out of `events`. `typing` — a text
/// field has the focus: keys that edit text are left to it.
pub fn take_actions(events: &mut Vec<egui::Event>, typing: bool) -> Vec<Action> {
    with(|map| {
        let mut out = Vec::new();
        events.retain(|e| {
            let egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } = e else { return true };
            // on a Cyrillic layout letters arrive as their physical (Latin) key too
            let found = [Some(*key), *physical_key].into_iter().flatten().find_map(|k| {
                let c = Combo::from_event(k, *modifiers);
                if typing && c.types_text() {
                    return None;
                }
                map.action_for(&c)
            });
            match found {
                Some(a) => {
                    out.push(a);
                    false
                }
                None => true,
            }
        });
        out
    })
}

/// What a key event means while a new combination is being recorded.
pub fn captured(events: &[egui::Event]) -> Option<Combo> {
    events.iter().find_map(|e| match e {
        egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } => {
            // letters on a non-Latin layout: record the Latin key, as it is matched
            let key = match physical_key {
                Some(p) if p.name().len() == 1 && key.name().len() != 1 => *p,
                _ => *key,
            };
            Some(Combo::from_event(key, *modifiers))
        }
        // egui-winit turns these into clipboard events and drops the key
        egui::Event::Cut => Some(Combo::new(false, false, true, Key::Delete)),
        egui::Event::Copy => Some(Combo::new(true, false, false, Key::C)),
        egui::Event::Paste(_) => Some(Combo::new(true, false, false, Key::V)),
        _ => None,
    })
}

// ───────────────────────────── settings page ─────────────────────────────

/// What the shortcuts page is doing: recording keys, asking about a conflict.
#[derive(Clone, Default)]
pub struct EditState {
    /// Recording keys for command `.0`: replace its key number `.1`, or add one.
    pub capture: Option<(usize, Option<usize>)>,
    /// The recorded combination already belongs to another command: (command,
    /// slot, combination, its current owner) — waiting for the user's answer.
    pub conflict: Option<(usize, Option<usize>, Combo, usize)>,
    /// Why the last recorded combination was not taken, under that command.
    pub error: Option<(usize, String)>,
    /// «Reset all» asked, waiting for confirmation.
    pub confirm_reset: bool,
}

impl EditState {
    pub fn capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// A combination was pressed while recording.
    pub fn on_capture(&mut self, combo: Combo, map: &mut Keymap) {
        let Some((cmd, slot)) = self.capture.take() else { return };
        self.error = None;
        if combo == k(Key::Escape) {
            return;
        }
        if let Some(why) = reserved(&combo) {
            self.error = Some((cmd, quokka_domain::trf!("{} нельзя назначить: эта клавиша всегда означает «{}»", "{} cannot be assigned: this key always means “{}”", combo.label(), why)));
            return;
        }
        match map.owner(&combo) {
            Some(o) if o == cmd => {}
            Some(o) => self.conflict = Some((cmd, slot, combo, o)),
            None => map.assign(cmd, combo, slot),
        }
    }
}

/// Whether the shortcuts page has something for a settings search.
pub fn matches(q: &str) -> bool {
    tr!("горячие клавиши сочетания клавиш клавиатура переназначить сбросить", "keyboard shortcuts hotkeys keys bindings reassign reset").contains(q)
        || with(|m| m.commands.iter().zip(&m.keys).any(|(c, k)| row_matches(c, k, q)))
}

fn row_matches(c: &Command, keys: &[Combo], q: &str) -> bool {
    q.is_empty() || c.name.to_lowercase().contains(q) || keys.iter().any(|k| k.label().to_lowercase().contains(q))
}

/// A key cap: «Ctrl+Shift+E». `active` — recording into it.
fn chip(ui: &mut egui::Ui, text: &str, active: bool, changed: bool) -> egui::Response {
    use crate::theme::*;
    use egui::{vec2, Sense, Stroke};
    let font = super::semibold(12.5);
    let col = if active { BLUE } else { TEXT() };
    let g = ui.painter().layout_no_wrap(text.to_string(), font, col);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 18.0, 26.0), Sense::click());
    let (fill, stroke) = if active {
        (BLUE_SOFT(), Stroke::new(1.5, BLUE))
    } else if resp.hovered() {
        (BLUE_SOFT(), Stroke::new(1.0, BLUE))
    } else if changed {
        (WARN_BG(), Stroke::new(1.0, WARN_BORDER()))
    } else {
        (INPUT_BG(), Stroke::new(1.0, FIELD_BORDER()))
    };
    ui.painter().rect(r, rad(5.0), fill, stroke, eframe::egui::StrokeKind::Middle);
    ui.painter().galley(r.center() - g.size() / 2.0, g, col);
    resp
}


/// The «Keyboard shortcuts» settings page. Edits `overrides` (what is saved).
pub fn page(ui: &mut egui::Ui, overrides: &mut BTreeMap<String, String>, st: &mut EditState, q: &str) {
    use crate::theme::*;
    use egui::{vec2, RichText};
    let mut map = Keymap::with_overrides(overrides);
    ui.spacing_mut().item_spacing.y = 2.0;
    super::jb_comment(ui, 0.0, tr!(
        "Щёлкните сочетание, чтобы назначить другое, или «+», чтобы добавить ещё одно. Esc — отменить запись. Если сочетание уже занято, Quokka спросит, переназначить ли его. Стрелки, Enter, Esc, Tab, пробел и Ctrl+C / X / V не переназначаются.",
        "Click a shortcut to assign another one, or “+” to add one more. Esc cancels recording. If the combination is already taken, Quokka asks whether to reassign it. Arrows, Enter, Esc, Tab, Space and Ctrl+C / X / V cannot be reassigned."
    ));
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let can_reset = !map.is_default();
        if st.confirm_reset {
            ui.label(RichText::new(tr!("Вернуть все сочетания по умолчанию?", "Restore all default shortcuts?")).size(13.5).color(TEXT()));
            if super::jb_btn(ui, tr!("Сбросить", "Reset"), true).clicked() {
                map = Keymap::defaults();
                *st = EditState::default();
            }
            if super::jb_btn(ui, tr!("Отмена", "Cancel"), false).clicked() {
                st.confirm_reset = false;
            }
        } else if ui.add_enabled(can_reset, egui::Button::new(RichText::new(tr!("Сбросить все по умолчанию", "Reset all to defaults")).size(13.5))).clicked() {
            st.confirm_reset = true;
        }
    });

    let mut group = None;
    for i in 0..map.commands.len() {
        let (cmd, keys) = (&map.commands[i], map.keys[i].clone());
        if !row_matches(cmd, &keys, q) {
            continue;
        }
        if group != Some(cmd.group) {
            group = Some(cmd.group);
            super::jb_group(ui, cmd.group.title());
        }
        let changed = cmd.defaults != keys;
        let name = cmd.name;
        let defaults = cmd.defaults.clone();
        let mut action: Option<Box<dyn FnOnce(&mut Keymap, &mut EditState)>> = None;
        // name on the left, keys on the right; the tool buttons keep fixed columns
        // (empty ones reserve their place) so the keys line up from row to row
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.set_min_height(28.0);
            let hit = !q.is_empty() && name.to_lowercase().contains(q);
            let label = RichText::new(name).size(13.5).color(TEXT());
            let label = if hit { label.background_color(super::SEARCH_HIT()) } else { label };
            ui.add(egui::Label::new(label).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let tool = vec2(26.0, 26.0);
                if changed {
                    let tip = if defaults.is_empty() {
                        tr!("Сбросить: без сочетания", "Reset: no shortcut").to_string()
                    } else {
                        quokka_domain::trf!("Сбросить: {}", "Reset: {}", defaults.iter().map(Combo::label).collect::<Vec<_>>().join(", "))
                    };
                    if super::jb_tool(ui, super::Glyph::Refresh, true, &tip).clicked() {
                        action = Some(Box::new(move |m, st| {
                            let defaults = m.commands[i].defaults.clone();
                            m.keys[i].clear();
                            for c in defaults {
                                // a default now used by another command stays there: ask about it
                                match m.owner(&c) {
                                    Some(o) if o != i => st.conflict = Some((i, None, c, o)),
                                    _ => m.keys[i].push(c),
                                }
                            }
                        }));
                    }
                } else {
                    ui.allocate_exact_size(tool, egui::Sense::hover());
                }
                if keys.is_empty() {
                    ui.allocate_exact_size(tool, egui::Sense::hover());
                } else if super::jb_tool(ui, super::Glyph::Close, true, tr!("Убрать все сочетания команды", "Remove all shortcuts of this command")).clicked() {
                    action = Some(Box::new(move |m, _| m.keys[i].clear()));
                }
                let adding = st.capture == Some((i, None));
                if adding {
                    chip(ui, tr!("Нажмите сочетание…", "Press keys…"), true, false);
                } else if super::jb_tool(ui, super::Glyph::Plus, true, tr!("Добавить сочетание", "Add shortcut")).clicked() {
                    action = Some(Box::new(move |_, st| *st = EditState { capture: Some((i, None)), ..Default::default() }));
                }
                for (slot, combo) in keys.iter().enumerate().rev() {
                    let recording = st.capture == Some((i, Some(slot)));
                    let text = if recording { tr!("Нажмите сочетание…", "Press keys…").to_string() } else { combo.label() };
                    let r = chip(ui, &text, recording, changed);
                    if r.clicked() {
                        action = Some(Box::new(move |_, st| *st = EditState { capture: Some((i, Some(slot))), ..Default::default() }));
                    }
                    r.context_menu(|ui| {
                        if ui.button(tr!("Убрать это сочетание", "Remove this shortcut")).clicked() {
                            let c = *combo;
                            action = Some(Box::new(move |m, _| m.keys[i].retain(|k| *k != c)));
                            ui.close();
                        }
                    });
                    r.on_hover_text(tr!("Щёлкните, чтобы назначить другое сочетание; правый щелчок — убрать", "Click to assign another shortcut; right-click to remove"));
                }
                if keys.is_empty() && !adding {
                    ui.label(RichText::new(tr!("нет", "none")).size(12.5).color(MUTED()));
                }
            });
        });
        if let Some((cmd_i, text)) = &st.error {
            if *cmd_i == i {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(64.0);
                    ui.label(RichText::new(text).size(12.5).color(RED));
                });
            }
        }
        if let Some((cmd_i, slot, combo, owner)) = st.conflict {
            if cmd_i == i {
                egui::Frame::NONE
                    .fill(WARN_BG())
                    .stroke(egui::Stroke::new(1.0, WARN_BORDER()))
                    .corner_radius(rad(6.0))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let owner_name = map.commands[owner].name;
                        ui.add(egui::Label::new(RichText::new(quokka_domain::trf!(
                            "{} уже назначено команде «{}». Переназначить на «{}»? У «{}» это сочетание будет убрано.",
                            "{} is already assigned to “{}”. Reassign it to “{}”? It will be removed from “{}”.",
                            combo.label(), owner_name, name, owner_name
                        )).size(13.0).color(TEXT())).wrap());
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if super::jb_btn(ui, tr!("Переназначить", "Reassign"), true).clicked() {
                                action = Some(Box::new(move |m, st| {
                                    m.assign(cmd_i, combo, slot);
                                    st.conflict = None;
                                }));
                            }
                            if super::jb_btn(ui, tr!("Отмена", "Cancel"), false).clicked() {
                                action = Some(Box::new(|_, st| st.conflict = None));
                            }
                        });
                    });
            }
        }
        if let Some(f) = action {
            f(&mut map, st);
        }
    }
    if group.is_none() {
        ui.label(RichText::new(tr!("Ничего не найдено", "Nothing found")).size(13.0).color(MUTED()));
    }
    *overrides = map.overrides();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_round_trip_through_config_text() {
        for c in [ctrl_shift(Key::E), alt(Key::ArrowLeft), k(Key::F5), shift(Key::Delete), ctrl_alt(Key::Enter)] {
            assert_eq!(Combo::parse(&c.to_config()), Some(c));
        }
        assert_eq!(ctrl_shift(Key::E).label(), "Ctrl+Shift+E");
        assert_eq!(alt(Key::ArrowLeft).label(), "Alt+←");
        assert_eq!(Combo::parse("ctrl + shift + e"), Some(ctrl_shift(Key::E)));
        assert_eq!(Combo::parse("Ctrl+"), None);
    }

    #[test]
    fn defaults_have_no_conflicts_or_reserved_keys() {
        let map = Keymap::defaults();
        let mut seen = std::collections::HashSet::new();
        for (cmd, keys) in map.commands.iter().zip(&map.keys) {
            for c in keys {
                assert!(seen.insert(*c), "{} is bound twice", c.label());
                assert!(reserved(c).is_none(), "{} of {} is reserved", c.label(), cmd.id);
            }
        }
        let ids: std::collections::HashSet<_> = map.commands.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), map.commands.len(), "duplicate command id");
    }

    #[test]
    fn reassigning_moves_the_combo_and_survives_saving() {
        let mut map = Keymap::defaults();
        let copy = map.index("copy").unwrap();
        let view = map.index("view").unwrap();
        // F3 → Copy, replacing F5
        map.assign(copy, k(Key::F3), Some(0));
        assert_eq!(map.action_for(&k(Key::F3)), Some(Action::Copy));
        assert!(map.keys[view].is_empty());
        assert_eq!(map.action_for(&k(Key::F5)), None);
        let saved = map.overrides();
        assert_eq!(saved.get("copy").map(String::as_str), Some("F3"));
        assert_eq!(saved.get("view").map(String::as_str), Some(""));
        let loaded = Keymap::with_overrides(&saved);
        assert_eq!(loaded.keys, map.keys);
        assert!(Keymap::with_overrides(&BTreeMap::new()).is_default());
    }

    #[test]
    fn saved_combo_wins_over_a_default_of_another_command() {
        let mut saved = BTreeMap::new();
        saved.insert("refresh".to_string(), "F5".to_string());
        let map = Keymap::with_overrides(&saved);
        assert_eq!(map.action_for(&k(Key::F5)), Some(Action::Refresh));
        assert!(map.keys[map.index("copy").unwrap()].is_empty());
    }

    #[test]
    fn key_events_become_actions_except_text_keys_while_typing() {
        install(Keymap::defaults());
        let ev = |key, modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let mut events = vec![ev(Key::F7, Modifiers::ALT), ev(Key::F7, Modifiers::NONE), ev(Key::Q, Modifiers::NONE)];
        assert_eq!(take_actions(&mut events, false), vec![Action::FindFiles, Action::MkDir]);
        assert_eq!(events.len(), 1, "unbound keys stay for the widgets");
        let mut events = vec![ev(Key::Delete, Modifiers::NONE), ev(Key::A, Modifiers::COMMAND), ev(Key::F5, Modifiers::NONE)];
        assert_eq!(take_actions(&mut events, true), vec![Action::Copy]);
        // Cyrillic layout: the logical key is unknown, the physical one is S
        let mut events = vec![egui::Event::Key { key: Key::F35, physical_key: Some(Key::S), pressed: true, repeat: false, modifiers: Modifiers::COMMAND }];
        assert_eq!(take_actions(&mut events, false), vec![Action::SelectMask]);
    }

    #[test]
    fn alt_enter_is_not_confused_with_enter_or_ctrl_alt_enter() {
        install(Keymap::defaults());
        let ev = |modifiers| egui::Event::Key { key: Key::Enter, physical_key: Some(Key::Enter), pressed: true, repeat: false, modifiers };
        let mut events = vec![ev(Modifiers::ALT)];
        assert_eq!(take_actions(&mut events, false), vec![Action::CalculateSizes]);
        assert!(events.is_empty(), "the plain Enter handler must not see it");
        let mut events = vec![ev(Modifiers::ALT)];
        assert_eq!(take_actions(&mut events, true), vec![Action::CalculateSizes], "works from the path box too");
        let mut events = vec![ev(Modifiers::COMMAND | Modifiers::ALT)];
        assert_eq!(take_actions(&mut events, false), vec![Action::Properties]);
        let mut events = vec![ev(Modifiers::NONE)];
        assert!(take_actions(&mut events, false).is_empty(), "plain Enter stays «open»");
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn reserved_keys() {
        assert!(reserved(&ctrl(Key::C)).is_some());
        assert!(reserved(&k(Key::Enter)).is_some());
        assert!(reserved(&k(Key::ArrowDown)).is_some());
        assert!(reserved(&alt(Key::ArrowLeft)).is_none());
        assert!(reserved(&ctrl_shift(Key::C)).is_none());
    }
}
