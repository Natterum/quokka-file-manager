//! Persistent settings stored as a plain text file in the user's config folder.

use quokka_domain::tr;
use crate::icons::Glyph;
use crate::theme::{Appearance, ColorPreset, ThemeMode};
use quokka_domain::lang::Lang;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: String,
    /// Empty path means the virtual "Этот компьютер" view.
    pub path: PathBuf,
    pub glyph: Glyph,
    /// Custom icon (see `iconsrc`); when set it replaces `glyph`.
    pub icon: Option<String>,
    /// Colour for the built-in glyph; `None` = theme default.
    pub color: Option<[u8; 3]>,
}

impl Place {
    pub fn new(name: impl Into<String>, path: PathBuf, glyph: Glyph) -> Place {
        Place { name: name.into(), path, glyph, icon: None, color: None }
    }
}

fn color_to_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn color_from_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub const COLUMN_COUNT: usize = 8;

/// Column ids: name, modified, size, type, created, accessed, extension, attributes. Name is always visible.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnLayout {
    pub order: [usize; COLUMN_COUNT],
    pub visible: [bool; COLUMN_COUNT],
}

impl Default for ColumnLayout {
    fn default() -> Self {
        Self { order: std::array::from_fn(|id| id), visible: [true, true, true, true, false, false, false, false] }
    }
}

impl ColumnLayout {
    /// Insert the dragged column at a boundary in the original order.
    pub fn move_column(&mut self, from: usize, boundary: usize) {
        if from >= COLUMN_COUNT || boundary > COLUMN_COUNT { return; }
        let to = if boundary > from { boundary - 1 } else { boundary };
        if to > from { self.order[from..=to].rotate_left(1); }
        else if to < from { self.order[to..=from].rotate_right(1); }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub show_hidden: bool,
    pub associated_icons: bool,
    pub dual: bool,
    pub regex: bool,
    pub case_sensitive: bool,
    pub show_drives: bool,
    pub sidebar_labels: bool,
    /// External text editor command for F4; empty = system default.
    pub editor: String,
    pub places: Vec<Place>,
    /// Recent masks and texts of the file search (Alt+F7), newest first.
    pub find_masks: Vec<String>,
    pub find_texts: Vec<String>,
    /// Widths by column id minus one; «Имя» takes the rest.
    pub col_widths: [[f32; COLUMN_COUNT - 1]; 2],
    pub columns: [ColumnLayout; 2],
    /// Recent name masks of the group rename (Ctrl+M), newest first.
    pub rename_masks: Vec<String>,
    /// Reopen the tabs of the last session on start.
    pub restore_session: bool,
    /// Icon in the notification area (system tray).
    pub tray_icon: bool,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    /// Allow several Quokka windows; otherwise a new launch brings up the running one.
    pub multi_instance: bool,
    /// Interface language.
    pub language: Lang,
    /// Open tabs at the last exit.
    pub session: Session,
    /// Saved server connections (SFTP / FTP / FTPS). Passwords are never stored here —
    /// see `save_password`.
    pub connections: Vec<Connection>,
    /// Look for a new version on GitHub at start-up (at most once a day).
    pub check_updates: bool,
    /// Unix time of the last successful update check.
    pub update_checked_at: i64,
    /// Version the user chose to skip; not offered again by the automatic check.
    pub update_skip: String,
    /// Keyboard shortcuts changed by the user: command id → "Ctrl+R, F5"
    /// (empty — no keys). Commands not listed keep their defaults.
    pub hotkeys: std::collections::BTreeMap<String, String>,
    /// Font, corner rounding and colour theme.
    pub appearance: Appearance,
}

/// A saved server connection.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Connection {
    /// Shown in the list; empty — the address is shown.
    pub name: String,
    /// `sftp://user@host[:port]`, `ftp://…` or `ftps://…`.
    pub url: String,
    /// Log in with this key (SFTP); empty — with a password.
    pub key_file: String,
    /// The password (or key passphrase) is kept in the Windows Credential Manager.
    pub save_password: bool,
}

impl Connection {
    pub fn title(&self) -> &str {
        if self.name.trim().is_empty() { &self.url } else { &self.name }
    }
}

/// Open tabs of both panes, restored on the next start.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Session {
    /// Folders of the tabs of the left and right pane (empty path = «Этот компьютер»).
    pub tabs: [Vec<PathBuf>; 2],
    /// Current tab of each pane.
    pub cur: [usize; 2],
    /// Active pane.
    pub active: usize,
}

pub const DEFAULT_COL_WIDTHS: [f32; COLUMN_COUNT - 1] = [170.0, 100.0, 150.0, 170.0, 170.0, 100.0, 110.0];
pub const MIN_COL_WIDTH: f32 = 64.0;

/// Keep at most this many entries in each search history list.
pub const HISTORY_LEN: usize = 15;

/// Move `item` to the front of a history list.
pub fn remember(list: &mut Vec<String>, item: &str) {
    let item = item.trim();
    if item.is_empty() {
        return;
    }
    list.retain(|s| s != item);
    list.insert(0, item.to_string());
    list.truncate(HISTORY_LEN);
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn home() -> PathBuf {
    home_dir()
}

fn known_folder(home: &PathBuf, name: &str) -> Option<PathBuf> {
    [home.join(name), home.join("OneDrive").join(name)].into_iter().find(|p| p.is_dir())
}

/// Standard favourites: (path, Russian name, English name, glyph).
fn standard_places() -> Vec<(PathBuf, &'static str, &'static str, Glyph)> {
    let home = home_dir();
    let mut places = Vec::new();
    if cfg!(windows) {
        places.push((PathBuf::new(), "Этот компьютер", "This PC", Glyph::Desktop));
    }
    places.push((home.clone(), "Домашняя папка", "Home folder", Glyph::Home));
    for (dir, ru, en, glyph) in [
        ("Desktop", "Рабочий стол", "Desktop", Glyph::Columns),
        ("Documents", "Документы", "Documents", Glyph::Document),
        ("Downloads", "Загрузки", "Downloads", Glyph::Download),
        ("Pictures", "Изображения", "Pictures", Glyph::Image),
        ("Music", "Музыка", "Music", Glyph::Music),
        ("Videos", "Видео", "Videos", Glyph::Video),
    ] {
        if let Some(path) = known_folder(&home, dir) {
            places.push((path, ru, en, glyph));
        }
    }
    places
}

fn pick(ru: &'static str, en: &'static str) -> &'static str {
    if quokka_domain::lang::is_en() { en } else { ru }
}

pub fn default_places() -> Vec<Place> {
    standard_places().into_iter().map(|(path, ru, en, glyph)| Place::new(pick(ru, en), path, glyph)).collect()
}

/// After a language switch: standard favourites that still have their default
/// name get it in the current language; names the user changed stay.
pub fn relabel_standard_places(places: &mut [Place]) {
    let std = standard_places();
    for pl in places {
        if let Some((_, ru, en, _)) = std.iter().find(|(path, ru, en, _)| pl.path == *path && (pl.name == *ru || pl.name == *en)) {
            pl.name = pick(ru, en).to_string();
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            show_hidden: false,
            associated_icons: false,
            dual: true,
            regex: false,
            case_sensitive: false,
            show_drives: true,
            sidebar_labels: false,
            editor: String::new(),
            places: default_places(),
            find_masks: Vec::new(),
            find_texts: Vec::new(),
            col_widths: [DEFAULT_COL_WIDTHS; 2],
            columns: std::array::from_fn(|_| ColumnLayout::default()),
            rename_masks: Vec::new(),
            restore_session: true,
            tray_icon: true,
            close_to_tray: false,
            multi_instance: false,
            language: system_language(),
            session: Session::default(),
            connections: Vec::new(),
            check_updates: true,
            update_checked_at: 0,
            update_skip: String::new(),
            hotkeys: Default::default(),
            appearance: Appearance::default(),
        }
    }
}

/// Language of the Windows user interface (or `LANG` elsewhere): Russian for a
/// Russian system, English for any other.
pub fn system_language() -> Lang {
    #[cfg(windows)]
    let ru = {
        // LANG_RUSSIAN in the low 10 bits of the UI language id
        let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        id & 0x3ff == 0x19
    };
    #[cfg(not(windows))]
    let ru = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .is_some_and(|v| v.starts_with("ru"));
    if ru { Lang::Ru } else { Lang::En }
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("Quokka").join("settings.txt"))
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The settings file exists but could not be read at start-up: saving must not
/// replace it with the defaults this process fell back to.
static UNREADABLE: AtomicBool = AtomicBool::new(false);

/// What this process last read from or wrote to the settings file: its modification
/// time and the addresses of the saved connections in it.
static DISK: Mutex<Option<(Option<SystemTime>, Vec<String>)>> = Mutex::new(None);

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("bak")
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Read a settings file. Right after an update or at log-on the file can be held
/// for a moment by an antivirus or the indexer, so a failed read is retried before
/// giving up; a missing file is reported at once. Invalid UTF-8 is replaced, not refused.
fn read_text(path: &Path) -> std::io::Result<String> {
    let mut tries = 0;
    loop {
        match fs::read(path) {
            Ok(bytes) => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(e),
            Err(e) => {
                tries += 1;
                if tries >= 30 {
                    return Err(e);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Whether `text` has at least one line Quokka writes (an imported file must).
fn looks_like_settings(text: &str) -> bool {
    text.lines().any(|l| {
        let key = l.split('\t').next().unwrap_or("");
        matches!(key, "show_hidden" | "dual" | "places" | "place" | "connection" | "language" | "theme" | "hotkey")
    })
}

fn remember_disk(path: &Path, s: &Settings) {
    let urls = s.connections.iter().map(|c| c.url.clone()).collect();
    *DISK.lock().unwrap_or_else(|e| e.into_inner()) = Some((modified(path), urls));
}

impl Settings {
    pub fn load() -> Settings {
        let Some(path) = config_path() else { return Settings::default() };
        let main = read_text(&path);
        let s = match &main {
            Ok(text) if !text.trim().is_empty() => Self::from_text(text),
            _ => {
                if matches!(&main, Err(e) if e.kind() != std::io::ErrorKind::NotFound) {
                    UNREADABLE.store(true, Ordering::Relaxed);
                }
                // missing, empty or locked: the copy kept by the previous save
                match read_text(&backup_path(&path)) {
                    Ok(text) if looks_like_settings(&text) => Self::from_text(&text),
                    _ => Settings::default(),
                }
            }
        };
        remember_disk(&path, &s);
        s
    }

    /// Read settings exported from another computer (or a copy of `settings.txt`).
    pub fn import(path: &Path) -> Result<Settings, String> {
        let text = read_text(path).map_err(|e| e.to_string())?;
        if !looks_like_settings(&text) {
            return Err(tr!("Это не файл настроек Quokka", "This is not a Quokka settings file").into());
        }
        Ok(Self::from_text(&text))
    }

    /// Write the settings to `path` for moving them to another computer.
    pub fn export(&self, path: &Path) -> Result<(), String> {
        fs::write(path, self.to_text()).map_err(|e| e.to_string())
    }

    /// Server connections saved to the file by another Quokka window (or by the
    /// previous version during an update) since this process last read or wrote it
    /// are added, so this save does not drop them. Connections this process knew
    /// and removed stay removed.
    fn merge_from_disk(&mut self, path: &Path) {
        let disk = DISK.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some((stamp, known)) = disk else { return };
        if modified(path) == stamp {
            return;
        }
        let Ok(text) = read_text(path) else { return };
        for c in Self::from_text(&text).connections {
            if !known.contains(&c.url) && !self.connections.iter().any(|o| o.url == c.url) {
                self.connections.push(c);
            }
        }
    }

    /// Save to `settings.txt`: written to a temporary file first and then put in
    /// place, so a crash or a killed process never leaves a half-written file; the
    /// previous version is kept as `settings.bak`.
    pub fn save(&mut self) -> Result<(), String> {
        let Some(path) = config_path() else { return Err(tr!("Не найдена папка настроек", "Settings folder not found").into()) };
        if UNREADABLE.load(Ordering::Relaxed) {
            if read_text(&path).is_ok_and(|t| !t.trim().is_empty()) {
                return Err(tr!(
                    "Файл настроек не читался при запуске — он не перезаписан. Перезапустите Quokka.",
                    "The settings file could not be read at start-up, so it was not overwritten. Restart Quokka."
                )
                .into());
            }
            UNREADABLE.store(false, Ordering::Relaxed);
        }
        self.merge_from_disk(&path);
        let out = self.to_text();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        {
            use std::io::Write;
            let mut f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
            f.write_all(out.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
        }
        if fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
            let _ = fs::copy(&path, backup_path(&path));
        }
        let mut tries = 0;
        while let Err(e) = fs::rename(&tmp, &path) {
            tries += 1;
            if tries >= 20 {
                let _ = fs::remove_file(&tmp);
                return Err(e.to_string());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        remember_disk(&path, self);
        Ok(())
    }

    fn from_text(text: &str) -> Self {
        let mut s = Settings::default();
        let mut places = Vec::new();
        let mut saw_places = false;
        for line in text.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            let flag = |v: &str| v == "1";
            match parts.as_slice() {
                ["show_hidden", v] => s.show_hidden = flag(v),
                ["associated_icons", v] => s.associated_icons = flag(v),
                ["dual", v] => s.dual = flag(v),
                ["regex", v] => s.regex = flag(v),
                ["case_sensitive", v] => s.case_sensitive = flag(v),
                ["show_drives", v] => s.show_drives = flag(v),
                ["sidebar_labels", v] => s.sidebar_labels = flag(v),
                ["editor", v] => s.editor = unesc(v),
                ["restore_session", v] => s.restore_session = flag(v),
                ["tray_icon", v] => s.tray_icon = flag(v),
                ["close_to_tray", v] => s.close_to_tray = flag(v),
                ["multi_instance", v] => s.multi_instance = flag(v),
                ["check_updates", v] => s.check_updates = flag(v),
                ["update_checked_at", v] => s.update_checked_at = v.parse().unwrap_or(0),
                ["update_skip", v] => s.update_skip = unesc(v),
                ["hotkey", id, keys] => {
                    s.hotkeys.insert(unesc(id), unesc(keys));
                }
                ["hotkey", id] => {
                    s.hotkeys.insert(unesc(id), String::new());
                }
                ["language", v] => s.language = Lang::from_key(v).unwrap_or(s.language),
                ["font", v] if !v.is_empty() => s.appearance.font = unesc(v),
                ["rounding", v] => s.appearance.rounding = v.parse::<u8>().map_or(100, |r| r.min(100)),
                ["theme", v] => s.appearance.theme = ThemeMode::from_key(v).unwrap_or_default(),
                ["preset_light", v] => s.appearance.light_preset = ColorPreset::from_key(v).unwrap_or_default(),
                ["preset_dark", v] => s.appearance.dark_preset = ColorPreset::from_key(v).unwrap_or_default(),
                ["rename_mask", v] if s.rename_masks.len() < HISTORY_LEN => s.rename_masks.push(unesc(v)),
                ["session_active", v] => s.session.active = usize::from(*v == "1"),
                [key @ ("session_tab_left" | "session_tab_right"), v] => {
                    let pane = usize::from(*key == "session_tab_right");
                    s.session.tabs[pane].push(PathBuf::from(unesc(v)));
                }
                [key @ ("session_cur_left" | "session_cur_right"), v] => {
                    let pane = usize::from(*key == "session_cur_right");
                    s.session.cur[pane] = v.parse().unwrap_or(0);
                }
                [key @ ("columns_left" | "columns_right"), order, visible] => {
                    let pane = usize::from(*key == "columns_right");
                    if let Ok(ids) = order.split(',').map(str::parse::<usize>).collect::<Result<Vec<_>, _>>() {
                        if [4, COLUMN_COUNT].contains(&ids.len()) && (0..ids.len()).all(|id| ids.contains(&id)) {
                            s.columns[pane].order[..ids.len()].copy_from_slice(&ids);
                        }
                    }
                    if [4, COLUMN_COUNT].contains(&visible.len()) && visible.bytes().all(|b| b == b'0' || b == b'1') {
                        for (id, value) in visible.bytes().enumerate() {
                            s.columns[pane].visible[id] = id == 0 || value == b'1';
                        }
                    }
                }
                ["places"] => saw_places = true,
                ["col_widths", a, b, c] => {
                    for (i, v) in [a, b, c].into_iter().enumerate() {
                        if let Ok(w) = v.parse::<f32>() {
                            if w.is_finite() {
                                for widths in &mut s.col_widths {
                                    widths[i] = w.clamp(MIN_COL_WIDTH, 2000.0);
                                }
                            }
                        }
                    }
                }
                [key @ ("col_widths_left" | "col_widths_right"), values @ ..] => {
                    let pane = usize::from(*key == "col_widths_right");
                    for (i, v) in values.iter().take(COLUMN_COUNT - 1).enumerate() {
                        if let Ok(w) = v.parse::<f32>() {
                            if w.is_finite() {
                                s.col_widths[pane][i] = w.clamp(MIN_COL_WIDTH, 2000.0);
                            }
                        }
                    }
                }
                ["connection", name, url, key, save] => s.connections.push(Connection {
                    name: unesc(name), url: unesc(url), key_file: unesc(key), save_password: *save == "1",
                }),
                ["find_mask", v] if s.find_masks.len() < HISTORY_LEN => s.find_masks.push(unesc(v)),
                ["find_text", v] if s.find_texts.len() < HISTORY_LEN => s.find_texts.push(unesc(v)),
                ["place", g, name, path, rest @ ..] => {
                    saw_places = true;
                    let mut pl = Place::new(unesc(name), PathBuf::from(unesc(path)), Glyph::from_key(g));
                    pl.icon = rest.first().map(|v| unesc(v)).filter(|v| !v.is_empty());
                    pl.color = rest.get(1).and_then(|v| color_from_hex(v));
                    places.push(pl);
                }
                _ => {}
            }
        }
        if saw_places {
            s.places = places;
        }
        s
    }

    fn to_text(&self) -> String {
        let b = |v: bool| if v { "1" } else { "0" };
        let mut out = format!(
            "show_hidden\t{}\nassociated_icons\t{}\ndual\t{}\nregex\t{}\ncase_sensitive\t{}\nshow_drives\t{}\nsidebar_labels\t{}\neditor\t{}\nplaces\n",
            b(self.show_hidden),
            b(self.associated_icons),
            b(self.dual),
            b(self.regex),
            b(self.case_sensitive),
            b(self.show_drives),
            b(self.sidebar_labels),
            esc(&self.editor)
        );
        for (key, w) in ["col_widths_left", "col_widths_right"].iter().zip(self.col_widths) {
            out.push_str(&format!("{key}\t{}\n", w.map(|v| format!("{v:.0}")).join("\t")));
        }
        for (key, layout) in ["columns_left", "columns_right"].iter().zip(&self.columns) {
            let order = layout.order.map(|id| id.to_string()).join(",");
            let visible: String = layout.visible.iter().enumerate()
                .map(|(id, v)| if id == 0 || *v { '1' } else { '0' }).collect();
            out.push_str(&format!("{key}\t{order}\t{visible}\n"));
        }
        out.push_str(&format!(
            "restore_session	{}
tray_icon	{}
close_to_tray	{}
multi_instance	{}
language	{}
check_updates	{}
update_checked_at	{}
update_skip	{}
",
            b(self.restore_session),
            b(self.tray_icon),
            b(self.close_to_tray),
            b(self.multi_instance),
            self.language.key(),
            b(self.check_updates),
            self.update_checked_at,
            esc(&self.update_skip)
        ));
        out.push_str(&format!(
            "font\t{}\nrounding\t{}\ntheme\t{}\npreset_light\t{}\npreset_dark\t{}\n",
            esc(&self.appearance.font),
            self.appearance.rounding.min(100),
            self.appearance.theme.key(),
            self.appearance.light_preset.key(),
            self.appearance.dark_preset.key()
        ));
        for m in &self.rename_masks {
            out.push_str(&format!("rename_mask	{}
", esc(m)));
        }
        out.push_str(&format!("session_active	{}
", self.session.active.min(1)));
        for (pane, key) in ["left", "right"].iter().enumerate() {
            for t in &self.session.tabs[pane] {
                out.push_str(&format!("session_tab_{key}	{}
", esc(&t.display().to_string())));
            }
            out.push_str(&format!("session_cur_{key}	{}
", self.session.cur[pane]));
        }
        for (id, keys) in &self.hotkeys {
            out.push_str(&format!("hotkey\t{}\t{}\n", esc(id), esc(keys)));
        }
        for c in &self.connections {
            out.push_str(&format!("connection\t{}\t{}\t{}\t{}\n", esc(&c.name), esc(&c.url), esc(&c.key_file), b(c.save_password)));
        }
        for m in &self.find_masks {
            out.push_str(&format!("find_mask\t{}\n", esc(m)));
        }
        for t in &self.find_texts {
            out.push_str(&format!("find_text\t{}\n", esc(t)));
        }
        for p in &self.places {
            out.push_str(&format!(
                "place\t{}\t{}\t{}\t{}\t{}\n",
                p.glyph.key(),
                esc(&p.name),
                esc(&p.path.display().to_string()),
                esc(p.icon.as_deref().unwrap_or("")),
                p.color.map(color_to_hex).unwrap_or_default()
            ));
        }
        out
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_round_trip() {
        let mut s = Settings::default();
        s.hotkeys.insert("copy".into(), "F3, Ctrl+Shift+C".into());
        s.hotkeys.insert("view".into(), String::new());
        assert_eq!(Settings::from_text(&s.to_text()).hotkeys, s.hotkeys);
    }

    #[test]
    fn connections_round_trip() {
        let mut s = Settings::default();
        s.connections = vec![
            Connection { name: "Сайт\tпрод".into(), url: "sftp://bob@host:2222".into(), key_file: r"C:\keys\id ed".into(), save_password: false },
            Connection { name: String::new(), url: "ftps://root@h".into(), key_file: String::new(), save_password: true },
        ];
        assert_eq!(Settings::from_text(&s.to_text()).connections, s.connections);
        assert_eq!(s.connections[1].title(), "ftps://root@h");
    }

    #[test]
    fn associated_icons_default_and_round_trip() {
        let d = Settings::from_text("");
        assert!(d.check_updates && d.update_checked_at == 0 && d.update_skip.is_empty());
        let mut s = Settings::default();
        s.check_updates = false;
        s.update_checked_at = 1_763_700_000;
        s.update_skip = "0.2.0".into();
        let r = Settings::from_text(&s.to_text());
        assert_eq!((r.check_updates, r.update_checked_at, r.update_skip.as_str()), (false, 1_763_700_000, "0.2.0"));
        assert!(!Settings::from_text("").associated_icons);
        let enabled = Settings::from_text("associated_icons\t1\n");
        assert!(enabled.associated_icons);
        assert!(Settings::from_text(&enabled.to_text()).associated_icons);
        assert!(!Settings::from_text("associated_icons\t0\n").associated_icons);
    }

    #[test]
    fn old_column_layout_migrates_and_new_columns_round_trip() {
        let mut settings = Settings::from_text("columns_left\t3,0,2,1\t1010\ncol_widths_left\t200\t120\t180\n");
        assert_eq!(settings.columns[0].order, [3, 0, 2, 1, 4, 5, 6, 7]);
        assert_eq!(settings.columns[0].visible, [true, false, true, false, false, false, false, false]);
        settings.columns[0].order = [4, 0, 5, 6, 7, 1, 2, 3];
        settings.columns[0].visible = [true; COLUMN_COUNT];
        settings.col_widths[0][5] = 210.0;
        let restored = Settings::from_text(&settings.to_text());
        assert_eq!(restored.columns, settings.columns);
        assert_eq!(restored.col_widths, settings.col_widths);
    }

    #[test]
    fn drag_insertion_works_at_every_boundary() {
        for from in 0..COLUMN_COUNT {
            for boundary in 0..=COLUMN_COUNT {
                let mut layout = ColumnLayout::default();
                layout.move_column(from, boundary);
                let mut expected: Vec<_> = (0..COLUMN_COUNT).collect();
                let moved = expected.remove(from);
                expected.insert(if boundary > from { boundary - 1 } else { boundary }, moved);
                assert_eq!(layout.order.as_slice(), expected);
                assert_eq!(layout.visible, ColumnLayout::default().visible);
            }
        }
    }

    #[test]
    fn column_layouts_round_trip_independently() {
        let mut settings = Settings::from_text("");
        assert_eq!(settings.columns, [ColumnLayout::default(), ColumnLayout::default()]);
        settings.columns[0].order = [3, 2, 0, 1, 4, 5, 6, 7];
        settings.columns[0].visible = [true, false, true, false, true, false, true, false];
        settings.columns[1].order = [1, 0, 2, 3, 4, 5, 6, 7];
        assert_eq!(Settings::from_text(&settings.to_text()).columns, settings.columns);
    }

    #[test]
    fn invalid_layouts_are_ignored_and_name_cannot_be_hidden() {
        for order in ["0,1,1,3", "4,1,2,3", "0,1,2", "0,x,1,2,3"] {
            let settings = Settings::from_text(&format!("columns_left\t{order}\t0000\n"));
            assert_eq!(settings.columns[0].order, [0, 1, 2, 3, 4, 5, 6, 7]);
            assert_eq!(settings.columns[0].visible, [true, false, false, false, false, false, false, false]);
        }
        let settings = Settings::from_text("columns_right\t3,2,1,0\tbad\n");
        assert_eq!(settings.columns[1].order, [3, 2, 1, 0, 4, 5, 6, 7]);
        assert_eq!(settings.columns[1].visible, ColumnLayout::default().visible);
    }

    #[test]
    fn column_widths_migrate_and_round_trip_independently() {
        let mut settings = Settings::from_text("col_widths\t180\t110\t160\n");
        assert_eq!(settings.col_widths, [[180.0, 110.0, 160.0, 170.0, 170.0, 100.0, 110.0]; 2]);
        settings.col_widths[0][1] = 240.0;
        let restored = Settings::from_text(&settings.to_text());
        assert_eq!(restored.col_widths[0], [180.0, 240.0, 160.0, 170.0, 170.0, 100.0, 110.0]);
        assert_eq!(restored.col_widths[1], [180.0, 110.0, 160.0, 170.0, 170.0, 100.0, 110.0]);
    }

    #[test]
    fn window_options_and_session_round_trip() {
        let d = Settings::from_text("");
        assert!(d.restore_session && d.tray_icon && !d.close_to_tray && !d.multi_instance);
        let mut s = Settings::default();
        s.close_to_tray = true;
        s.multi_instance = true;
        s.tray_icon = false;
        s.rename_masks = vec!["[N]_[C]".into()];
        s.session = Session {
            tabs: [vec![PathBuf::from(r"C:\Users\a b"), PathBuf::new()], vec![PathBuf::from("D:\\")]],
            cur: [1, 0],
            active: 1,
        };
        let r = Settings::from_text(&s.to_text());
        assert_eq!(r.session, s.session);
        assert_eq!(r.rename_masks, s.rename_masks);
        assert!(r.close_to_tray && r.multi_instance && !r.tray_icon);
    }

    #[test]
    fn appearance_default_and_round_trip() {
        assert_eq!(Settings::from_text("").appearance, Appearance::default());
        let mut s = Settings::default();
        s.appearance = Appearance {
            font: "segoe".into(),
            rounding: 35,
            theme: ThemeMode::Dark,
            light_preset: ColorPreset::Forest,
            dark_preset: ColorPreset::Lavender,
        };
        assert_eq!(Settings::from_text(&s.to_text()).appearance, s.appearance);
        let bad = Settings::from_text("rounding\t250\ntheme\tneon\npreset_light\tneon\n").appearance;
        assert_eq!((bad.rounding, bad.theme, bad.light_preset), (100, ThemeMode::Light, ColorPreset::Classic));
    }

    #[test]
    fn import_needs_a_settings_file_and_reads_bad_utf8() {
        let dir = std::env::temp_dir().join(format!("quokka-config-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut s = Settings::default();
        s.connections = vec![Connection { name: "Прод".into(), url: "sftp://bob@host".into(), key_file: String::new(), save_password: true }];
        let good = dir.join("export.txt");
        s.export(&good).unwrap();
        assert_eq!(Settings::import(&good).unwrap().connections, s.connections);
        let other = dir.join("notes.txt");
        fs::write(&other, "hello\nworld\n").unwrap();
        assert!(Settings::import(&other).is_err());
        // a broken byte does not cost the whole file
        let mut bytes = s.to_text().into_bytes();
        bytes.splice(0..0, b"\xff\xfe junk\n".iter().copied());
        fs::write(&other, bytes).unwrap();
        assert_eq!(Settings::import(&other).unwrap().connections, s.connections);
        assert!(matches!(read_text(&dir.join("missing.txt")), Err(e) if e.kind() == std::io::ErrorKind::NotFound));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn language_round_trip() {
        for lang in Lang::ALL {
            let mut s = Settings::default();
            s.language = lang;
            assert_eq!(Settings::from_text(&s.to_text()).language, lang);
        }
        assert_eq!(Settings::from_text("language	ru
").language, Lang::Ru);
        assert_eq!(Settings::from_text("language	en
").language, Lang::En);
    }

    #[test]
    fn invalid_column_widths_are_ignored_or_clamped() {
        let settings = Settings::from_text("col_widths_left\tNaN\t-20\t99999\ncol_widths_right\tinf\t120\t150\n");
        assert_eq!(settings.col_widths[0], [170.0, MIN_COL_WIDTH, 2000.0, 170.0, 170.0, 100.0, 110.0]);
        assert_eq!(settings.col_widths[1], [170.0, 120.0, 150.0, 170.0, 170.0, 100.0, 110.0]);
    }
}
