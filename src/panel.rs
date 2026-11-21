//! One file-list tab: directory listing, cursor, marks, sorting, filtering and history.

use crate::util::FileKind;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub lower: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_parent: bool,
    pub is_drive: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub kind: FileKind,
    pub ext: String,
    pub hidden: bool,
    pub type_name: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Modified,
    Size,
    Type,
}

pub struct Panel {
    /// Empty path means the virtual "This PC" view (list of drives).
    pub path: PathBuf,
    pub all: Vec<Entry>,
    pub visible: Vec<usize>,
    pub cursor: usize,
    pub marked: HashSet<PathBuf>,
    pub filter: String,
    /// Treat the filter as a regular expression.
    pub filter_regex: bool,
    pub filter_case: bool,
    /// Set when the regular expression in the filter does not compile.
    pub filter_error: Option<String>,
    pub sort: SortKey,
    pub asc: bool,
    pub show_hidden: bool,
    pub error: Option<String>,
    pub dirs: usize,
    pub files: usize,
    pub bytes: u64,
    pub disk: Option<(u64, u64)>,
    pub scroll_offset: f32,
    pub view_height: f32,
    pub ensure_visible: bool,
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
    pub editing_path: Option<String>,
    pub focus_path_edit: bool,
    /// Mouse rubber-band selection: whether it is active, marks that existed before it
    /// (kept when Ctrl is held) and the start point in content coordinates.
    pub selecting: bool,
    pub drag_base: HashSet<PathBuf>,
    pub drag_start_y: f32,
    /// Search results fed to the panel (Alt+F7 → «Файлы в панель»): the tab lists these
    /// paths instead of the folder; `path` stays the folder the search started in.
    pub results: Option<Vec<PathBuf>>,
    pub results_label: String,
    pub folder_sizes: crate::folder_size::FolderSizes,
}

pub fn list_drives() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        (b'C'..=b'Z')
            .map(|c| PathBuf::from(format!("{}:\\", c as char)))
            .filter(|p| p.exists())
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![PathBuf::from("/")]
    }
}

/// Short label for a root such as `C:\` → `C:`.
pub fn root_label(p: &Path) -> String {
    let s = p.display().to_string();
    let t = s.trim_end_matches(['\\', '/']);
    if t.is_empty() {
        s
    } else {
        t.to_string()
    }
}

/// Parent of a path; the parent of a drive root is "This PC" (empty path) on Windows.
pub fn parent_of(path: &Path) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    match path.parent() {
        Some(p) => Some(p.to_path_buf()),
        None if cfg!(windows) => Some(PathBuf::new()),
        None => None,
    }
}

fn type_name(ext: &str, kind: FileKind) -> String {
    let up = ext.to_uppercase();
    match ext {
        "" => "Файл".into(),
        "md" => "Markdown".into(),
        "pdf" => "Документ PDF".into(),
        "txt" => "Текстовый документ".into(),
        "log" => "Файл журнала".into(),
        "doc" | "docx" | "odt" | "rtf" => "Документ Word".into(),
        "xls" | "xlsx" | "ods" | "csv" => "Электронная таблица".into(),
        "ppt" | "pptx" | "odp" => "Презентация".into(),
        "exe" | "app" | "com" => "Приложение".into(),
        "dll" | "sys" => "Системный файл".into(),
        "msi" => "Установщик".into(),
        "rs" => "Исходный код Rust".into(),
        "lnk" => "Ярлык".into(),
        _ => match kind {
            FileKind::Image => format!("Изображение {up}"),
            FileKind::Audio => format!("Аудио {up}"),
            FileKind::Video => format!("Видео {up}"),
            FileKind::Archive => format!("Архив {up}"),
            FileKind::Code => format!("Исходный код {up}"),
            _ => format!("Файл {up}"),
        },
    }
}

#[cfg(windows)]
fn is_hidden(name: &str, m: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    name.starts_with('.') || m.file_attributes() & 0x2 != 0
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _m: &fs::Metadata) -> bool {
    name.starts_with('.')
}

impl Panel {
    pub fn new(path: PathBuf, show_hidden: bool) -> Self {
        let mut p = Panel {
            path,
            all: Vec::new(),
            visible: Vec::new(),
            cursor: 0,
            marked: HashSet::new(),
            filter: String::new(),
            filter_regex: false,
            filter_case: false,
            filter_error: None,
            sort: SortKey::Name,
            asc: true,
            show_hidden,
            error: None,
            dirs: 0,
            files: 0,
            bytes: 0,
            disk: None,
            scroll_offset: 0.0,
            view_height: 400.0,
            ensure_visible: true,
            back: Vec::new(),
            forward: Vec::new(),
            editing_path: None,
            focus_path_edit: false,
            selecting: false,
            drag_base: HashSet::new(),
            drag_start_y: 0.0,
            results: None,
            results_label: String::new(),
            folder_sizes: Default::default(),
        };
        p.load();
        p
    }

    pub fn is_pc(&self) -> bool {
        self.path.as_os_str().is_empty()
    }

    pub fn title(&self) -> String {
        if self.results.is_some() {
            return self.results_label.clone();
        }
        if self.is_pc() {
            return "Этот компьютер".into();
        }
        match self.path.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => root_label(&self.path),
        }
    }

    fn load(&mut self) {
        self.folder_sizes.clear();
        // `visible` holds indices into `all`; drop them together so nothing
        // (e.g. `current()` in apply_filter_sort) indexes the new, shorter list
        self.all.clear();
        self.visible.clear();
        self.error = None;
        self.dirs = 0;
        self.files = 0;
        self.bytes = 0;

        if let Some(paths) = self.results.clone() {
            self.load_results(&paths);
            return;
        }

        if self.is_pc() {
            for d in list_drives() {
                let name = root_label(&d);
                let total = fs2::total_space(&d).ok();
                self.all.push(Entry {
                    lower: name.to_lowercase(),
                    name,
                    path: d,
                    is_dir: true,
                    is_parent: false,
                    is_drive: true,
                    size: total.unwrap_or(0),
                    modified: None,
                    kind: FileKind::Folder,
                    ext: String::new(),
                    hidden: false,
                    type_name: "Локальный диск".into(),
                });
                self.dirs += 1;
            }
            self.disk = None;
            self.apply_filter_sort();
            return;
        }

        if let Some(parent) = parent_of(&self.path) {
            self.all.push(Entry {
                name: "..".into(),
                lower: "..".into(),
                path: parent,
                is_dir: true,
                is_parent: true,
                is_drive: false,
                size: 0,
                modified: None,
                kind: FileKind::Folder,
                ext: String::new(),
                hidden: false,
                type_name: String::new(),
            });
        }

        match fs::read_dir(&self.path) {
            Ok(rd) => {
                for de in rd.flatten() {
                    let path = de.path();
                    let name = de.file_name().to_string_lossy().into_owned();
                    // hidden/system flags belong to the link itself, so read them before following it
                    let link_hidden = de.metadata().map(|m| is_hidden(&name, &m)).unwrap_or(false);
                    let md = de
                        .metadata()
                        .and_then(|m| if m.file_type().is_symlink() { fs::metadata(&path) } else { Ok(m) });
                    let (is_dir, size, modified, hidden) = match &md {
                        Ok(m) => (m.is_dir(), if m.is_dir() { 0 } else { m.len() }, m.modified().ok(), link_hidden || is_hidden(&name, m)),
                        Err(_) => (false, 0, None, name.starts_with('.')),
                    };
                    if hidden && !self.show_hidden {
                        continue;
                    }
                    let ext = if is_dir {
                        String::new()
                    } else {
                        match name.rsplit_once('.') {
                            Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
                            _ => String::new(),
                        }
                    };
                    let kind = if is_dir { FileKind::Folder } else { FileKind::from_name(&name) };
                    let type_name = if is_dir { "Папка с файлами".into() } else { type_name(&ext, kind) };
                    if is_dir {
                        self.dirs += 1;
                    } else {
                        self.files += 1;
                        self.bytes += size;
                    }
                    self.all.push(Entry {
                        lower: name.to_lowercase(),
                        name,
                        path,
                        is_dir,
                        is_parent: false,
                        is_drive: false,
                        size,
                        modified,
                        kind,
                        ext,
                        hidden,
                        type_name,
                    });
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.disk = match (fs2::total_space(&self.path), fs2::available_space(&self.path)) {
            (Ok(t), Ok(f)) => Some((t, f)),
            _ => None,
        };
        self.apply_filter_sort();
    }

    /// Entries for search results: names are paths relative to the start folder,
    /// ".." leads back to that folder.
    fn load_results(&mut self, paths: &[PathBuf]) {
        self.all.push(Entry {
            name: "..".into(),
            lower: "..".into(),
            path: self.path.clone(),
            is_dir: true,
            is_parent: true,
            is_drive: false,
            size: 0,
            modified: None,
            kind: FileKind::Folder,
            ext: String::new(),
            hidden: false,
            type_name: String::new(),
        });
        for path in paths {
            let Ok(link) = fs::symlink_metadata(path) else { continue };
            let md = if link.file_type().is_symlink() { fs::metadata(path).unwrap_or(link.clone()) } else { link.clone() };
            let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = match path.strip_prefix(&self.path) {
                Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
                _ => path.display().to_string(),
            };
            let is_dir = md.is_dir();
            let ext = if is_dir {
                String::new()
            } else {
                match file_name.rsplit_once('.') {
                    Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
                    _ => String::new(),
                }
            };
            let kind = if is_dir { FileKind::Folder } else { FileKind::from_name(&file_name) };
            let size = if is_dir { 0 } else { md.len() };
            if is_dir {
                self.dirs += 1;
            } else {
                self.files += 1;
                self.bytes += size;
            }
            self.all.push(Entry {
                lower: name.to_lowercase(),
                name,
                path: path.clone(),
                is_dir,
                is_parent: false,
                is_drive: false,
                size,
                modified: md.modified().ok(),
                kind,
                type_name: if is_dir { "Папка с файлами".into() } else { type_name(&ext, kind) },
                ext,
                hidden: is_hidden(&file_name, &link),
            });
        }
        self.disk = None;
        self.apply_filter_sort();
    }

    /// Show search results in this tab (TC "Feed to listbox").
    pub fn show_results(&mut self, root: PathBuf, paths: Vec<PathBuf>, label: String) {
        if self.path != root {
            let prev = std::mem::replace(&mut self.path, root);
            self.back.push(prev);
            self.forward.clear();
        }
        self.results = Some(paths);
        self.results_label = label;
        self.marked.clear();
        self.filter.clear();
        self.cursor = 0;
        self.scroll_offset = 0.0;
        self.editing_path = None;
        self.selecting = false;
        self.load();
        self.set_cursor(1);
    }

    /// Re-read the directory, keeping cursor and marks where possible.
    pub fn reload(&mut self) {
        let cur = self.current().map(|e| e.name.clone());
        let off = self.scroll_offset;
        self.load();
        let existing: HashSet<PathBuf> = self.all.iter().map(|e| e.path.clone()).collect();
        self.marked.retain(|p| existing.contains(p));
        if let Some(n) = cur {
            self.select_name(&n);
        }
        self.scroll_offset = off;
    }

    pub fn apply_filter_sort(&mut self) {
        let cur = self.current().map(|e| e.path.clone());
        let (sort, asc) = (self.sort, self.asc);
        self.all.sort_by(|a, b| {
            b.is_parent.cmp(&a.is_parent).then(b.is_dir.cmp(&a.is_dir)).then_with(|| {
                let o = match sort {
                    SortKey::Name => a.lower.cmp(&b.lower),
                    SortKey::Size => a.size.cmp(&b.size).then(a.lower.cmp(&b.lower)),
                    SortKey::Modified => a.modified.cmp(&b.modified).then(a.lower.cmp(&b.lower)),
                    SortKey::Type => a.type_name.cmp(&b.type_name).then(a.lower.cmp(&b.lower)),
                };
                if asc {
                    o
                } else {
                    o.reverse()
                }
            })
        });
        self.filter_error = None;
        let raw = self.filter.trim();
        let matcher: Box<dyn Fn(&Entry) -> bool> = if raw.is_empty() {
            Box::new(|_| true)
        } else if self.filter_regex {
            match regex::RegexBuilder::new(raw).case_insensitive(!self.filter_case).size_limit(1 << 20).build() {
                Ok(re) => Box::new(move |e: &Entry| !e.is_parent && re.is_match(&e.name)),
                Err(err) => {
                    self.filter_error = Some(regex_error_text(&err));
                    Box::new(|e: &Entry| !e.is_parent && false)
                }
            }
        } else if self.filter_case {
            let f = raw.to_string();
            Box::new(move |e: &Entry| !e.is_parent && e.name.contains(&f))
        } else {
            let f = raw.to_lowercase();
            Box::new(move |e: &Entry| !e.is_parent && e.lower.contains(&f))
        };
        self.visible = (0..self.all.len()).filter(|&i| matcher(&self.all[i])).collect();
        drop(matcher);
        self.cursor = cur
            .and_then(|p| self.visible.iter().position(|&i| self.all[i].path == p))
            .unwrap_or(0);
        self.clamp_cursor();
        self.ensure_visible = true;
    }

    fn clamp_cursor(&mut self) {
        if self.visible.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.visible.len() {
            self.cursor = self.visible.len() - 1;
        }
    }

    pub fn set_sort(&mut self, key: SortKey) {
        if self.sort == key {
            self.asc = !self.asc;
        } else {
            self.sort = key;
            self.asc = true;
        }
        self.apply_filter_sort();
    }

    pub fn current(&self) -> Option<&Entry> {
        self.visible.get(self.cursor).and_then(|&i| self.all.get(i))
    }

    pub fn entry_at(&self, row: usize) -> Option<&Entry> {
        self.visible.get(row).and_then(|&i| self.all.get(i))
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let n = self.visible.len() as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, n - 1) as usize;
        self.ensure_visible = true;
    }

    pub fn set_cursor(&mut self, i: usize) {
        self.cursor = i;
        self.clamp_cursor();
        self.ensure_visible = true;
    }

    pub fn select_name(&mut self, name: &str) {
        if let Some(pos) = self.visible.iter().position(|&i| self.all[i].name.eq_ignore_ascii_case(name)) {
            self.set_cursor(pos);
        }
    }

    pub fn toggle_mark(&mut self, row: usize) {
        if let Some(e) = self.entry_at(row) {
            if e.is_parent || e.is_drive {
                return;
            }
            let p = e.path.clone();
            if !self.marked.remove(&p) {
                self.marked.insert(p);
            }
        }
    }

    pub fn mark_range(&mut self, a: usize, b: usize) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        for row in lo..=hi {
            if let Some(e) = self.entry_at(row) {
                if !e.is_parent && !e.is_drive {
                    self.marked.insert(e.path.clone());
                }
            }
        }
    }

    pub fn mark_all(&mut self) {
        let all: Vec<PathBuf> = self
            .visible
            .iter()
            .map(|&i| &self.all[i])
            .filter(|e| !e.is_parent && !e.is_drive)
            .map(|e| e.path.clone())
            .collect();
        if all.iter().all(|p| self.marked.contains(p)) {
            self.marked.clear();
        } else {
            self.marked.extend(all);
        }
    }

    pub fn calculate_sizes(&mut self, all: bool, ctx: eframe::egui::Context) -> usize {
        let targets = self.targets();
        let paths: Vec<PathBuf> = self.visible.iter().map(|&i| &self.all[i])
            .filter(|e| e.is_dir && !e.is_parent && !e.is_drive && (all || targets.contains(&e.path)))
            .map(|e| e.path.clone()).collect();
        let count = paths.len();
        for entry in &mut self.all {
            if paths.contains(&entry.path) { entry.size = 0; }
        }
        self.folder_sizes.start(paths, ctx);
        count
    }

    pub fn poll_sizes(&mut self) {
        let updates = self.folder_sizes.poll();
        if updates.is_empty() { return; }
        for (path, size) in updates {
            for entry in &mut self.all {
                if entry.path == path { entry.size = size.bytes; }
            }
        }
        if self.sort == SortKey::Size { self.apply_filter_sort(); }
    }

    pub fn marked_size(&self) -> u64 {
        self.all.iter().filter(|e| self.marked.contains(&e.path)).map(|e| e.size).sum()
    }

    /// Items an operation applies to: marked ones, or the one under the cursor.
    pub fn targets(&self) -> Vec<PathBuf> {
        if !self.marked.is_empty() {
            return self
                .all
                .iter()
                .filter(|e| self.marked.contains(&e.path))
                .map(|e| e.path.clone())
                .collect();
        }
        match self.current() {
            Some(e) if !e.is_parent && !e.is_drive => vec![e.path.clone()],
            _ => Vec::new(),
        }
    }

    fn go(&mut self, path: PathBuf, record: bool) -> Result<(), String> {
        if !path.as_os_str().is_empty() {
            fs::read_dir(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
        }
        let was_results = self.results.take().is_some();
        let prev = std::mem::replace(&mut self.path, path);
        if record && prev != self.path {
            self.back.push(prev.clone());
            self.forward.clear();
        }
        self.marked.clear();
        self.filter.clear();
        self.cursor = 0;
        self.scroll_offset = 0.0;
        self.editing_path = None;
        self.selecting = false;
        self.load();
        if was_results {
            // leaving search results: nothing to re-select
        } else if let Ok(rest) = prev.strip_prefix(&self.path) {
            if let Some(first) = rest.components().next() {
                let name = first.as_os_str().to_string_lossy().into_owned();
                self.select_name(&name);
            }
        }
        self.ensure_visible = true;
        Ok(())
    }

    pub fn navigate(&mut self, path: PathBuf) -> Result<(), String> {
        self.go(path, true)
    }

    pub fn go_up(&mut self) -> Result<(), String> {
        if self.results.is_some() {
            return self.navigate(self.path.clone());
        }
        match parent_of(&self.path) {
            Some(p) => self.navigate(p),
            None => Ok(()),
        }
    }

    pub fn go_back(&mut self) -> Result<(), String> {
        if let Some(p) = self.back.pop() {
            let cur = self.path.clone();
            if let Err(e) = self.go(p, false) {
                return Err(e);
            }
            self.forward.push(cur);
        }
        Ok(())
    }

    pub fn go_forward(&mut self) -> Result<(), String> {
        if let Some(p) = self.forward.pop() {
            let cur = self.path.clone();
            if let Err(e) = self.go(p, false) {
                return Err(e);
            }
            self.back.push(cur);
        }
        Ok(())
    }

    pub fn duplicate(&self) -> Panel {
        let mut p = Panel::new(self.path.clone(), self.show_hidden);
        p.filter_regex = self.filter_regex;
        p.filter_case = self.filter_case;
        p.sort = self.sort;
        p.asc = self.asc;
        if self.results.is_some() {
            p.results = self.results.clone();
            p.results_label = self.results_label.clone();
            p.load();
        }
        p.apply_filter_sort();
        p
    }
}

fn regex_error_text(err: &regex::Error) -> String {
    match err {
        regex::Error::CompiledTooBig(_) => "Слишком сложное регулярное выражение".into(),
        regex::Error::Syntax(msg) => {
            let last = msg.lines().rev().find(|l| l.starts_with("error:")).unwrap_or("").trim_start_matches("error:").trim();
            if last.is_empty() {
                "Ошибка в регулярном выражении".into()
            } else {
                format!("Ошибка в регулярном выражении: {last}")
            }
        }
        _ => "Ошибка в регулярном выражении".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_sizes_are_per_tab_sortable_and_cleared_on_reload() {
        let root = std::env::temp_dir().join(format!("quokka-panel-sizes-{}", std::process::id()));
        fs::create_dir_all(root.join("small")).unwrap();
        fs::create_dir_all(root.join("large/nested")).unwrap();
        fs::write(root.join("small/a"), b"x").unwrap();
        fs::write(root.join("large/nested/a"), b"12345").unwrap();
        let mut left = Panel::new(root.clone(), false);
        let right = Panel::new(root.clone(), false);
        left.select_name("small");
        left.marked.insert(root.join("small"));
        let ctx = eframe::egui::Context::default();
        assert_eq!(left.calculate_sizes(false, ctx.clone()), 1);
        assert_eq!(left.calculate_sizes(true, ctx), 2);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !left.folder_sizes.pending.is_empty() {
            left.poll_sizes();
            assert!(std::time::Instant::now() < deadline, "folder scan timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(left.marked_size(), 1);
        assert!(right.folder_sizes.sizes.is_empty());
        left.set_sort(SortKey::Size);
        let folders: Vec<_> = left.all.iter().filter(|e| e.is_dir && !e.is_parent).collect();
        assert_eq!(folders[0].name, "small");
        assert_eq!(folders[1].size, 5);
        assert_eq!(left.current().unwrap().name, "small");
        left.reload();
        assert!(left.folder_sizes.sizes.is_empty());
        assert!(left.folder_sizes.pending.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    /// Regression: reloading after files vanished used stale row indices and panicked.
    #[test]
    fn reload_after_items_removed() {
        let dir = std::env::temp_dir().join("quokka-panel-reload-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for n in ["a.txt", "b.txt", "c.txt", "d.txt"] {
            fs::write(dir.join(n), b"x").unwrap();
        }
        let mut p = Panel::new(dir.clone(), false);
        p.set_cursor(usize::MAX);
        for n in ["b.txt", "c.txt", "d.txt"] {
            fs::remove_file(dir.join(n)).unwrap();
        }
        p.reload();
        assert_eq!(p.files, 1);
        assert!(p.current().is_some());
    }

    /// Search results fed to a tab show relative names; ".." / go_up returns to the folder.
    #[test]
    fn search_results_mode() {
        let dir = std::env::temp_dir().join("quokka-panel-results-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("x.txt"), b"x").unwrap();
        fs::write(dir.join("y.txt"), b"yy").unwrap();
        let mut p = Panel::new(dir.clone(), false);
        let found = vec![dir.join("sub").join("x.txt"), dir.join("y.txt"), dir.join("gone.txt")];
        p.show_results(dir.clone(), found, "Поиск: *.txt".into());
        assert_eq!(p.title(), "Поиск: *.txt");
        let names: Vec<&str> = p.visible.iter().map(|&i| p.all[i].name.as_str()).collect();
        assert_eq!(names, ["..", &format!("sub{}x.txt", std::path::MAIN_SEPARATOR), "y.txt"]);
        assert_eq!(p.bytes, 3);
        p.go_up().unwrap();
        assert!(p.results.is_none());
        assert_eq!(p.path, dir);
        assert!(p.all.iter().any(|e| e.name == "sub"));
        let _ = fs::remove_dir_all(&dir);
    }
}
