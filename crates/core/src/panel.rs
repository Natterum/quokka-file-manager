//! One file-list tab: listing, cursor, marks, sorting, filtering and history.
//!
//! Folder contents are read in the background (see [`crate::listing`]). Small
//! folders still appear within the same frame — [`Panel`] waits a few milliseconds
//! for the result — while big or slow ones show "Загрузка…" and keep the window
//! responsive. Actions that need the new list (select a name, put the cursor on a
//! row) are remembered and applied when it arrives.

use quokka_domain::tr;
use crate::folder_size::FolderSizes;
use crate::listing::{self, Loaded, Pending, Request, Source};
use crate::ports::{Services, WatchGuard};
use quokka_domain::entry::{self, Entry, Filter};
pub use quokka_domain::paths::{parent_of, root_label};
pub use quokka_domain::SortKey;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a tab blocks waiting for a listing before showing "Загрузка…".
const SYNC_WAIT: Duration = Duration::from_millis(30);
/// How long navigation waits for a folder to open before going ahead anyway.
const PROBE_WAIT: Duration = Duration::from_millis(400);
/// Minimal pause between automatic refreshes caused by changes on disk.
const MIN_AUTO_REFRESH: Duration = Duration::from_millis(250);

/// What to do once the listing in progress arrives.
#[derive(Default)]
struct AfterLoad {
    /// Put the cursor on the item with this name.
    select: Option<String>,
    /// Put the cursor on this row (if `select` is not set or not found).
    row: Option<usize>,
    /// Keep the cursor on the same item as before (reload).
    keep: Option<PathBuf>,
    scroll: Option<f32>,
    /// Make the list take `scroll` even though it shows a different folder now.
    force_scroll: bool,
    /// Drop marks of items that are gone.
    retain_marks: bool,
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
    pub folder_sizes: FolderSizes,
    /// The listing shown is out of date; refresh when the tab is next visible.
    pub stale: bool,

    services: Services,
    loading: Option<Pending>,
    after: AfterLoad,
    /// When the last listing finished and how long it took.
    loaded_at: Instant,
    load_cost: Duration,
    watch: Option<(PathBuf, WatchGuard)>,
    changed: Arc<AtomicBool>,
    wake_scheduled: bool,
    /// Where the list was scrolled and which item was current in folders visited
    /// before, so coming back to one shows it as it was left.
    views: HashMap<PathBuf, SavedView>,
}

/// How a folder looked when the tab left it.
struct SavedView {
    scroll: f32,
    cursor: Option<String>,
}

/// Folders whose view a tab remembers; beyond that the memory starts over.
const MAX_SAVED_VIEWS: usize = 500;

impl Panel {
    pub fn new(path: PathBuf, show_hidden: bool, services: Services) -> Self {
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
            stale: false,
            services,
            loading: None,
            after: AfterLoad::default(),
            loaded_at: Instant::now(),
            load_cost: Duration::ZERO,
            watch: None,
            changed: Arc::new(AtomicBool::new(false)),
            wake_scheduled: false,
            views: HashMap::new(),
        };
        p.load(false);
        p
    }

    pub fn is_pc(&self) -> bool {
        self.path.as_os_str().is_empty()
    }

    /// A listing is being read; the rows shown may be the previous ones or none.
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    pub fn title(&self) -> String {
        if self.results.is_some() {
            return self.results_label.clone();
        }
        if self.is_pc() {
            return tr!("Этот компьютер", "This PC").into();
        }
        match self.path.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => root_label(&self.path),
        }
    }

    fn request(&self) -> Request {
        let source = if let Some(paths) = &self.results {
            Source::Results { root: self.path.clone(), paths: paths.clone() }
        } else if self.is_pc() {
            Source::Pc
        } else {
            Source::Dir(self.path.clone())
        };
        Request { source, show_hidden: self.show_hidden, sort: self.sort, asc: self.asc }
    }

    /// Start reading the listing. `keep` — keep showing the current rows meanwhile
    /// (a refresh of the same folder); otherwise the list is emptied at once.
    fn load(&mut self, keep: bool) {
        self.folder_sizes.clear();
        self.error = None;
        self.stale = false;
        self.changed.store(false, Ordering::Relaxed);
        if !keep {
            // `visible` holds indices into `all`; drop them together so nothing
            // (e.g. `current()`) indexes the new, shorter list
            self.all.clear();
            self.visible.clear();
            self.dirs = 0;
            self.files = 0;
            self.bytes = 0;
            self.disk = None;
        }
        self.loading = Some(listing::start(&self.services, self.request()));
        self.finish_loading(Some(SYNC_WAIT));
    }

    /// Apply the listing if it has arrived (waiting up to `wait`).
    fn finish_loading(&mut self, wait: Option<Duration>) -> bool {
        let Some(pending) = &mut self.loading else { return false };
        let loaded = match wait {
            Some(t) => pending.wait(t),
            None => pending.try_take(),
        };
        match loaded {
            Some(l) => {
                self.loading = None;
                self.apply(l);
                true
            }
            None => false,
        }
    }

    fn apply(&mut self, l: Loaded) {
        let keep_path = self.after.keep.take().or_else(|| self.current().map(|e| e.path.clone()));
        self.all = l.entries;
        self.dirs = l.dirs;
        self.files = l.files;
        self.bytes = l.bytes;
        self.disk = l.disk;
        self.error = l.error;
        self.loaded_at = Instant::now();
        self.load_cost = l.cost;
        if l.sort != Some((self.sort, self.asc)) {
            // the sort order changed while the listing was being read
            entry::sort(&mut self.all, self.sort, self.asc);
        }
        let after = std::mem::take(&mut self.after);
        if after.retain_marks {
            let existing: HashSet<&Path> = self.all.iter().map(|e| e.path.as_path()).collect();
            self.marked.retain(|p| existing.contains(p.as_path()));
        }
        self.apply_filter_keep(keep_path.as_deref());
        if let Some(row) = after.row {
            self.set_cursor(row);
        }
        if let Some(name) = after.select {
            self.select_name(&name);
        }
        if let Some(off) = after.scroll {
            self.scroll_offset = off;
            if after.force_scroll {
                self.ensure_visible = true;
            }
        }
        self.update_watch();
    }

    /// Watch the shown folder so changes made by other programs appear on their own.
    fn update_watch(&mut self) {
        let want = (self.results.is_none() && !self.is_pc() && self.error.is_none()).then(|| self.path.clone());
        if self.watch.as_ref().map(|(p, _)| p) == want.as_ref() {
            return;
        }
        self.watch = None;
        let (Some(dir), Some(watcher)) = (want, self.services.watcher.clone()) else { return };
        let (flag, waker) = (self.changed.clone(), self.services.waker.clone());
        let guard = watcher.watch(
            &dir,
            Box::new(move || {
                if !flag.swap(true, Ordering::Relaxed) {
                    waker.wake();
                }
            }),
        );
        self.watch = guard.map(|g| (dir, g));
    }

    /// Per-frame housekeeping: pick up a finished listing and folder sizes, refresh
    /// after changes on disk. `shown` — the tab is on screen; hidden tabs only note
    /// that they are out of date and refresh when shown.
    pub fn poll(&mut self, shown: bool) {
        self.finish_loading(None);
        self.poll_sizes();
        if self.changed.load(Ordering::Relaxed) && self.loading.is_none() {
            if !shown {
                self.stale = true;
                self.changed.store(false, Ordering::Relaxed);
            } else {
                // big folders take longer to list, so they are refreshed less often
                let pause = MIN_AUTO_REFRESH.max(self.load_cost * 4);
                if self.loaded_at.elapsed() >= pause {
                    self.wake_scheduled = false;
                    self.reload();
                } else if !self.wake_scheduled {
                    // nothing else may repaint the window: wake it when the pause is over
                    self.wake_scheduled = true;
                    let waker = self.services.waker.clone();
                    let left = pause - self.loaded_at.elapsed();
                    std::thread::spawn(move || {
                        std::thread::sleep(left);
                        waker.wake();
                    });
                }
            }
        }
        if shown && self.stale && self.loading.is_none() {
            self.reload();
        }
    }

    /// Block until the listing in progress is applied (tests, scripted use).
    pub fn wait_loaded(&mut self) {
        while self.loading.is_some() {
            self.finish_loading(Some(Duration::from_millis(50)));
        }
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
        self.after = AfterLoad { row: Some(1), ..Default::default() };
        self.load(false);
    }

    /// Re-read the folder, keeping cursor, scroll position and marks where possible.
    /// The current rows stay on screen until the new listing arrives.
    pub fn reload(&mut self) {
        let keep = self.current().map(|e| e.path.clone());
        let select = self.current().map(|e| e.name.clone());
        self.after = AfterLoad { select, keep, scroll: Some(self.scroll_offset), retain_marks: true, row: None, force_scroll: false };
        self.load(true);
    }

    /// Re-sort and re-filter after the sort key, the filter or its options changed.
    pub fn apply_filter_sort(&mut self) {
        let cur = self.current().map(|e| e.path.clone());
        entry::sort(&mut self.all, self.sort, self.asc);
        self.apply_filter_keep(cur.as_deref());
    }

    /// Re-filter only (the filter text changed): cheaper than [`apply_filter_sort`].
    pub fn apply_filter(&mut self) {
        let cur = self.current().map(|e| e.path.clone());
        self.apply_filter_keep(cur.as_deref());
    }

    fn apply_filter_keep(&mut self, cur: Option<&Path>) {
        let (filter, error) = Filter::new(&self.filter, self.filter_regex, self.filter_case);
        self.filter_error = error;
        self.visible = match filter {
            Filter::All => (0..self.all.len()).collect(),
            f => (0..self.all.len()).filter(|&i| f.matches(&self.all[i])).collect(),
        };
        self.cursor = cur.and_then(|p| self.visible.iter().position(|&i| self.all[i].path == p)).unwrap_or(0);
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
        if self.loading.is_some() && self.all.is_empty() {
            self.after.row = Some(i);
            return;
        }
        self.cursor = i;
        self.clamp_cursor();
        self.ensure_visible = true;
    }

    /// Put the cursor on the item called `name`; if the listing is still being
    /// read, do it when it arrives.
    pub fn select_name(&mut self, name: &str) {
        if self.loading.is_some() {
            self.after.select = Some(name.to_string());
            self.after.keep = None;
            return;
        }
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

    /// Add/remove matching visible items without affecting marks outside the filter.
    pub fn mark_mask(&mut self, mask: &str, select: bool) -> Result<(), String> {
        let re = quokka_domain::selection::masks(mask)?;
        for &i in &self.visible {
            let e = &self.all[i];
            if !e.is_parent && !e.is_drive && re.is_match(e.path.file_name().unwrap_or_default().to_string_lossy().as_ref()) {
                if select { self.marked.insert(e.path.clone()); } else { self.marked.remove(&e.path); }
            }
        }
        Ok(())
    }

    pub fn invert_marks(&mut self) {
        for &i in &self.visible {
            let e = &self.all[i];
            if !e.is_parent && !e.is_drive && !self.marked.remove(&e.path) { self.marked.insert(e.path.clone()); }
        }
    }

    /// Start size scans of the marked (or current) folders, or of all visible ones.
    /// Returns how many folders are being scanned.
    pub fn calculate_sizes(&mut self, all: bool) -> usize {
        let targets: HashSet<PathBuf> = self.targets().into_iter().collect();
        let paths: Vec<PathBuf> = self
            .visible
            .iter()
            .map(|&i| &self.all[i])
            .filter(|e| e.is_dir && !e.is_parent && !e.is_drive && (all || targets.contains(&e.path)))
            .map(|e| e.path.clone())
            .collect();
        let count = paths.len();
        let set: HashSet<&PathBuf> = paths.iter().collect();
        for entry in &mut self.all {
            if set.contains(&entry.path) {
                entry.size = 0;
            }
        }
        self.folder_sizes.start(paths, &self.services);
        count
    }

    pub fn poll_sizes(&mut self) {
        let updates = self.folder_sizes.poll();
        if updates.is_empty() {
            return;
        }
        let index: HashMap<&Path, usize> = self.all.iter().enumerate().map(|(i, e)| (e.path.as_path(), i)).collect();
        let rows: Vec<(usize, u64)> =
            updates.iter().filter_map(|(path, size)| index.get(path.as_path()).map(|&i| (i, size.bytes))).collect();
        drop(index);
        for (i, bytes) in rows {
            self.all[i].size = bytes;
        }
        if self.sort == SortKey::Size {
            self.apply_filter_sort();
        }
    }

    pub fn marked_size(&self) -> u64 {
        self.all.iter().filter(|e| self.marked.contains(&e.path)).map(|e| e.size).sum()
    }

    /// Items an operation applies to: marked ones, or the one under the cursor.
    pub fn targets(&self) -> Vec<PathBuf> {
        if !self.marked.is_empty() {
            return self.all.iter().filter(|e| self.marked.contains(&e.path)).map(|e| e.path.clone()).collect();
        }
        match self.current() {
            Some(e) if !e.is_parent && !e.is_drive => vec![e.path.clone()],
            _ => Vec::new(),
        }
    }

    fn go(&mut self, path: PathBuf, record: bool) -> Result<(), String> {
        if !path.as_os_str().is_empty() {
            listing::probe_dir(&self.services, &path, PROBE_WAIT)?;
        }
        if self.results.is_none() && !self.all.is_empty() {
            if self.views.len() >= MAX_SAVED_VIEWS {
                self.views.clear();
            }
            let cursor = self.current().filter(|e| !e.is_parent).map(|e| e.name.clone());
            self.views.insert(self.path.clone(), SavedView { scroll: self.scroll_offset, cursor });
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
        self.after = AfterLoad::default();
        if !was_results {
            // coming up from a subfolder: put the cursor on it
            if let Ok(rest) = prev.strip_prefix(&self.path) {
                if let Some(first) = rest.components().next() {
                    self.after.select = Some(first.as_os_str().to_string_lossy().into_owned());
                }
            }
            // a folder seen before: same scroll position and, unless coming up from
            // a subfolder, the same current item
            if let Some(v) = self.views.get(&self.path) {
                self.after.scroll = Some(v.scroll);
                self.after.force_scroll = true;
                if self.after.select.is_none() {
                    self.after.select = v.cursor.clone();
                }
            }
        }
        self.load(false);
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
            self.go(p, false)?;
            self.forward.push(cur);
        }
        Ok(())
    }

    pub fn go_forward(&mut self) -> Result<(), String> {
        if let Some(p) = self.forward.pop() {
            let cur = self.path.clone();
            self.go(p, false)?;
            self.back.push(cur);
        }
        Ok(())
    }

    pub fn duplicate(&self) -> Panel {
        let mut p = Panel::new(self.path.clone(), self.show_hidden, self.services.clone());
        p.filter_regex = self.filter_regex;
        p.filter_case = self.filter_case;
        p.sort = self.sort;
        p.asc = self.asc;
        if self.results.is_some() {
            p.results = self.results.clone();
            p.results_label = self.results_label.clone();
            p.load(false);
        }
        p.apply_filter_sort();
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFs;

    #[test]
    fn mask_selection_and_inversion_respect_visible_items() {
        let fs = FakeFs::new();
        fs.file("/r/a.JPG", 1);
        fs.file("/r/b.txt", 1);
        fs.file("/r/c.png", 1);
        let mut p = panel(&fs, "/r");
        p.mark_mask("*.jpg;*.png", true).unwrap();
        assert_eq!(p.marked.len(), 2);
        p.invert_marks();
        assert_eq!(p.targets(), vec![PathBuf::from("/r/b.txt")]);
        p.mark_mask("*.txt", false).unwrap();
        assert!(p.marked.is_empty());
        p.mark_mask("*.png", true).unwrap();
        p.filter = "JPG".into();
        p.apply_filter();
        p.invert_marks();
        assert_eq!(p.marked.len(), 2, "marks outside the filter are preserved");
        assert!(p.marked.iter().all(|path| path.file_name().is_some()));
        let before = p.marked.clone();
        assert!(p.mark_mask(";", true).is_err());
        assert_eq!(p.marked, before);
    }

    fn panel(fs: &Arc<FakeFs>, path: &str) -> Panel {
        let mut p = Panel::new(PathBuf::from(path), false, fs.services());
        p.wait_loaded();
        p
    }

    #[test]
    fn coming_back_restores_scroll_and_cursor() {
        let fs = FakeFs::new();
        for i in 0..100 {
            fs.file(&format!("/r/d{i:03}/f"), 1);
        }
        fs.file("/other/x", 1);
        let mut p = panel(&fs, "/r");
        p.select_name("d080");
        p.scroll_offset = 1234.0;
        p.navigate(PathBuf::from("/r/d080")).unwrap();
        p.wait_loaded();
        assert_eq!(p.scroll_offset, 0.0, "a new folder starts at the top");
        p.ensure_visible = false;

        // up: cursor on the folder we came from, scroll as it was
        p.go_up().unwrap();
        p.wait_loaded();
        assert_eq!(p.current().map(|e| e.name.as_str()), Some("d080"));
        assert_eq!(p.scroll_offset, 1234.0);
        assert!(p.ensure_visible, "the list must be told to take the saved offset");

        // going back from an unrelated folder restores the current item too
        p.select_name("d010");
        p.scroll_offset = 50.0;
        p.navigate(PathBuf::from("/other")).unwrap();
        p.wait_loaded();
        p.go_back().unwrap();
        p.wait_loaded();
        assert_eq!(p.path, PathBuf::from("/r"));
        assert_eq!(p.current().map(|e| e.name.as_str()), Some("d010"));
        assert_eq!(p.scroll_offset, 50.0);
    }

    #[test]
    fn folder_sizes_are_per_tab_sortable_and_cleared_on_reload() {
        let fs = FakeFs::new();
        fs.file("/r/small/a", 1);
        fs.file("/r/large/nested/a", 5);
        let mut left = panel(&fs, "/r");
        let right = panel(&fs, "/r");
        left.select_name("small");
        left.marked.insert(PathBuf::from("/r/small"));
        assert_eq!(left.calculate_sizes(false), 1);
        assert_eq!(left.calculate_sizes(true), 2);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !left.folder_sizes.pending.is_empty() {
            left.poll_sizes();
            assert!(Instant::now() < deadline, "folder scan timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(left.marked_size(), 1);
        assert!(right.folder_sizes.sizes.is_empty());
        left.set_sort(SortKey::Size);
        let folders: Vec<_> = left.all.iter().filter(|e| e.is_dir && !e.is_parent).collect();
        assert_eq!(folders[0].name, "small");
        assert_eq!(folders[1].size, 5);
        assert_eq!(left.current().unwrap().name, "small");
        left.reload();
        left.wait_loaded();
        assert!(left.folder_sizes.sizes.is_empty());
        assert!(left.folder_sizes.pending.is_empty());
    }

    /// Regression: reloading after files vanished used stale row indices and panicked.
    #[test]
    fn reload_after_items_removed() {
        let fs = FakeFs::new();
        for n in ["a.txt", "b.txt", "c.txt", "d.txt"] {
            fs.file(&format!("/d/{n}"), 1);
        }
        let mut p = panel(&fs, "/d");
        p.set_cursor(usize::MAX);
        p.marked.insert(PathBuf::from("/d/c.txt"));
        for n in ["b.txt", "c.txt", "d.txt"] {
            fs.remove(&format!("/d/{n}"));
        }
        p.reload();
        p.wait_loaded();
        assert_eq!(p.files, 1);
        assert!(p.current().is_some());
        assert!(p.marked.is_empty(), "marks of removed items are dropped");
    }

    /// Search results fed to a tab show relative names; ".." / go_up returns to the folder.
    #[test]
    fn search_results_mode() {
        let fs = FakeFs::new();
        fs.file("/s/sub/x.txt", 1);
        fs.file("/s/y.txt", 2);
        let mut p = panel(&fs, "/s");
        let root = PathBuf::from("/s");
        let found = vec![root.join("sub").join("x.txt"), root.join("y.txt"), root.join("gone.txt")];
        p.show_results(PathBuf::from("/s"), found, "Поиск: *.txt".into());
        p.wait_loaded();
        assert_eq!(p.title(), "Поиск: *.txt");
        let names: Vec<&str> = p.visible.iter().map(|&i| p.all[i].name.as_str()).collect();
        assert_eq!(names, ["..", &format!("sub{}x.txt", std::path::MAIN_SEPARATOR), "y.txt"]);
        assert_eq!(p.bytes, 3);
        assert_eq!(p.cursor, 1);
        p.go_up().unwrap();
        p.wait_loaded();
        assert!(p.results.is_none());
        assert_eq!(p.path, PathBuf::from("/s"));
        assert!(p.all.iter().any(|e| e.name == "sub"));
    }

    /// A slow folder does not block: the tab shows "loading" and picks the result
    /// up later, applying a selection requested in the meantime.
    #[test]
    fn slow_listing_is_async_and_deferred_selection_applies() {
        let fs = FakeFs::new();
        fs.file("/slow/a", 1);
        fs.file("/slow/b", 1);
        let mut p = panel(&fs, "/");
        fs.set_delay(Duration::from_millis(300));
        let t0 = Instant::now();
        p.navigate(PathBuf::from("/slow")).unwrap();
        assert!(t0.elapsed() < Duration::from_millis(250), "navigation must not wait for the listing");
        assert!(p.is_loading());
        p.select_name("b");
        p.wait_loaded();
        assert_eq!(p.current().unwrap().name, "b");
    }

    /// A newer navigation wins over an older listing that finishes later.
    #[test]
    fn stale_listing_is_discarded() {
        let fs = FakeFs::new();
        fs.file("/one/x", 1);
        fs.file("/two/y", 1);
        let mut p = panel(&fs, "/");
        fs.set_delay(Duration::from_millis(200));
        p.navigate(PathBuf::from("/one")).unwrap();
        fs.set_delay(Duration::ZERO);
        p.navigate(PathBuf::from("/two")).unwrap();
        p.wait_loaded();
        std::thread::sleep(Duration::from_millis(300));
        p.poll(true);
        assert!(p.all.iter().any(|e| e.name == "y"));
        assert!(!p.all.iter().any(|e| e.name == "x"));
    }

    /// Changes reported by the watcher refresh the visible tab; hidden tabs only
    /// become stale and refresh once shown.
    #[test]
    fn watcher_refreshes_shown_tabs_and_marks_hidden_ones_stale() {
        let fs = FakeFs::new();
        fs.file("/w/a", 1);
        let mut shown = panel(&fs, "/w");
        let mut hidden = panel(&fs, "/w");
        fs.file("/w/b", 1);
        fs.notify("/w");
        std::thread::sleep(MIN_AUTO_REFRESH + Duration::from_millis(20));
        shown.poll(true);
        shown.wait_loaded();
        hidden.poll(false);
        assert_eq!(shown.files, 2);
        assert!(hidden.stale);
        assert_eq!(hidden.files, 1);
        hidden.poll(true);
        hidden.wait_loaded();
        assert_eq!(hidden.files, 2);
    }

    #[test]
    fn filter_changes_do_not_resort() {
        let fs = FakeFs::new();
        for n in ["alpha", "beta", "gamma"] {
            fs.file(&format!("/f/{n}"), 1);
        }
        let mut p = panel(&fs, "/f");
        p.filter = "a".into();
        p.apply_filter();
        let names: Vec<&str> = p.visible.iter().map(|&i| p.all[i].name.as_str()).collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
        p.filter = "mm".into();
        p.apply_filter();
        assert_eq!(p.visible.len(), 1);
    }
}
