//! Persistent settings stored as a plain text file in the user's config folder.

use quokka_domain::tr;
use crate::icons::Glyph;
use quokka_domain::lang::Lang;
use std::fs;
use std::path::PathBuf;

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

impl Settings {
    pub fn load() -> Settings {
        let Some(text) = config_path().and_then(|p| fs::read_to_string(p).ok()) else {
            return Settings::default();
        };
        Self::from_text(&text)
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
                ["language", v] => s.language = Lang::from_key(v).unwrap_or(s.language),
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

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = config_path() else { return Err(tr!("Не найдена папка настроек", "Settings folder not found").into()) };
        let out = self.to_text();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::write(&path, out).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
