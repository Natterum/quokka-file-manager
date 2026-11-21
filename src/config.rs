//! Persistent settings stored as a plain text file in the user's config folder.

use crate::icons::Glyph;
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
    /// Widths of the file-list columns «Изменён», «Размер», «Тип»; «Имя» takes the rest.
    pub col_widths: [[f32; 3]; 2],
}

pub const DEFAULT_COL_WIDTHS: [f32; 3] = [170.0, 100.0, 150.0];
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

pub fn default_places() -> Vec<Place> {
    let home = home_dir();
    let mut places = Vec::new();
    if cfg!(windows) {
        places.push(Place::new("Этот компьютер", PathBuf::new(), Glyph::Desktop));
    }
    places.push(Place::new("Домашняя папка", home.clone(), Glyph::Home));
    for (dir, name, glyph) in [
        ("Desktop", "Рабочий стол", Glyph::Columns),
        ("Documents", "Документы", Glyph::Document),
        ("Downloads", "Загрузки", Glyph::Download),
        ("Pictures", "Изображения", Glyph::Image),
        ("Music", "Музыка", Glyph::Music),
        ("Videos", "Видео", Glyph::Video),
    ] {
        if let Some(path) = known_folder(&home, dir) {
            places.push(Place::new(name, path, glyph));
        }
    }
    places
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
        }
    }
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
                [key @ ("col_widths_left" | "col_widths_right"), a, b, c] => {
                    let pane = usize::from(*key == "col_widths_right");
                    for (i, v) in [a, b, c].into_iter().enumerate() {
                        if let Ok(w) = v.parse::<f32>() {
                            if w.is_finite() {
                                s.col_widths[pane][i] = w.clamp(MIN_COL_WIDTH, 2000.0);
                            }
                        }
                    }
                }
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
            out.push_str(&format!("{key}\t{:.0}\t{:.0}\t{:.0}\n", w[0], w[1], w[2]));
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
        let Some(path) = config_path() else { return Err("Не найдена папка настроек".into()) };
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
    fn associated_icons_default_and_round_trip() {
        assert!(!Settings::from_text("").associated_icons);
        let enabled = Settings::from_text("associated_icons\t1\n");
        assert!(enabled.associated_icons);
        assert!(Settings::from_text(&enabled.to_text()).associated_icons);
        assert!(!Settings::from_text("associated_icons\t0\n").associated_icons);
    }

    #[test]
    fn column_widths_migrate_and_round_trip_independently() {
        let mut settings = Settings::from_text("col_widths\t180\t110\t160\n");
        assert_eq!(settings.col_widths, [[180.0, 110.0, 160.0]; 2]);
        settings.col_widths[0][1] = 240.0;
        let restored = Settings::from_text(&settings.to_text());
        assert_eq!(restored.col_widths[0], [180.0, 240.0, 160.0]);
        assert_eq!(restored.col_widths[1], [180.0, 110.0, 160.0]);
    }

    #[test]
    fn invalid_column_widths_are_ignored_or_clamped() {
        let settings = Settings::from_text("col_widths_left\tNaN\t-20\t99999\ncol_widths_right\tinf\t120\t150\n");
        assert_eq!(settings.col_widths[0], [170.0, MIN_COL_WIDTH, 2000.0]);
        assert_eq!(settings.col_widths[1], [170.0, 120.0, 150.0]);
    }
}
