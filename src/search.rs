//! Background file search (Alt+F7), modelled on Total Commander's "Find files":
//! wildcard masks with exclusions, regular expressions, text inside files (several
//! encodings), size / date / kind filters, depth limit and a look inside ZIP archives.

use crate::zip::ZipReader;
use eframe::egui;
use regex::bytes::{Regex as BytesRegex, RegexBuilder as BytesRegexBuilder};
use regex::{Regex, RegexBuilder};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kinds {
    All,
    Files,
    Dirs,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SizeCmp {
    Less,
    Equal,
    Greater,
}

#[derive(Clone, Debug)]
pub struct TextQuery {
    pub text: String,
    pub case: bool,
    pub words: bool,
    pub regex: bool,
    /// Find files that do NOT contain the text.
    pub not_containing: bool,
    /// Also look for the text encoded as UTF-16 LE and Windows-1251.
    pub other_encodings: bool,
}

#[derive(Clone, Debug)]
pub struct Query {
    pub roots: Vec<PathBuf>,
    /// Total Commander style mask (`*.txt;*.doc | temp* bak\`) or a regex.
    pub mask: String,
    pub mask_regex: bool,
    pub mask_case: bool,
    /// `None` — all subfolders, `Some(0)` — only the start folder.
    pub depth: Option<usize>,
    pub kinds: Kinds,
    pub hidden: bool,
    pub archives: bool,
    pub text: Option<TextQuery>,
    /// Comparison, value, unit in bytes.
    pub size: Option<(SizeCmp, u64, u64)>,
    pub newer_than: Option<SystemTime>,
    pub older_than: Option<SystemTime>,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    /// Path of the item inside a ZIP archive (`path` is the archive then).
    pub inner: Option<String>,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Line with the first match when searching for text.
    pub snippet: Option<String>,
}

impl Hit {
    /// Name shown in the list: the file name, or the last part of the archive entry.
    pub fn name(&self) -> String {
        match &self.inner {
            Some(inner) => inner.trim_end_matches('/').rsplit('/').next().unwrap_or(inner).to_string(),
            None => self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.path.display().to_string()),
        }
    }

    /// Folder the item lives in, for display.
    pub fn location(&self) -> String {
        match &self.inner {
            Some(inner) => {
                let dir = inner.trim_end_matches('/').rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                if dir.is_empty() {
                    self.path.display().to_string()
                } else {
                    format!("{} › {}", self.path.display(), dir.replace('/', "\\"))
                }
            }
            None => self.path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
        }
    }
}

enum Msg {
    Found(Vec<Hit>),
    Progress { current: String, files: u64, dirs: u64 },
    Done { errors: u64 },
}

pub struct Search {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    pub hits: Vec<Hit>,
    pub current: String,
    pub files: u64,
    pub dirs: u64,
    pub errors: u64,
    pub running: bool,
    pub cancelled: bool,
    started: Instant,
    pub elapsed: Duration,
}

impl Search {
    /// Validate the query, compile its patterns and start the worker thread.
    pub fn start(q: Query, ctx: egui::Context) -> Result<Search, String> {
        if q.roots.is_empty() {
            return Err("Укажите, где искать".into());
        }
        for r in &q.roots {
            if !r.is_dir() {
                return Err(format!("Папка не найдена: {}", r.display()));
            }
        }
        let names = NameMatcher::new(&q.mask, q.mask_regex, q.mask_case)?;
        let text = match &q.text {
            Some(t) if !t.text.is_empty() => Some((text_regex(t)?, t.not_containing)),
            Some(_) => return Err("Введите текст для поиска".into()),
            None => None,
        };
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut w = Worker {
            q,
            names,
            text,
            tx,
            ctx,
            cancel: cancel.clone(),
            batch: Vec::new(),
            last: Instant::now(),
            files: 0,
            dirs: 0,
            errors: 0,
            current: String::new(),
        };
        thread::spawn(move || w.run());
        Ok(Search {
            rx,
            cancel,
            hits: Vec::new(),
            current: String::new(),
            files: 0,
            dirs: 0,
            errors: 0,
            running: true,
            cancelled: false,
            started: Instant::now(),
            elapsed: Duration::ZERO,
        })
    }

    pub fn stop(&mut self) {
        if self.running {
            self.cancel.store(true, Ordering::Relaxed);
            self.cancelled = true;
        }
    }

    /// Take everything the worker has sent so far.
    pub fn poll(&mut self) {
        if self.running {
            self.elapsed = self.started.elapsed();
        }
        loop {
            match self.rx.try_recv() {
                Ok(Msg::Found(h)) => self.hits.extend(h),
                Ok(Msg::Progress { current, files, dirs }) => {
                    self.current = current;
                    self.files = files;
                    self.dirs = dirs;
                }
                Ok(Msg::Done { errors }) => {
                    self.errors = errors;
                    self.running = false;
                    self.elapsed = self.started.elapsed();
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.running = false;
                    break;
                }
            }
        }
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

// ───────────────────────────── name masks ─────────────────────────────

struct NameMatcher {
    include: Option<Regex>,
    exclude_files: Option<Regex>,
    exclude_dirs: Option<Regex>,
}

/// Split a mask list on `;` and whitespace; double quotes keep names with spaces together.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            ';' | ' ' | '\t' | ',' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `*.txt` → `^.*\.txt$`; a mask without wildcards matches any name containing it.
fn wildcard(tok: &str) -> String {
    let tok = if tok == "*.*" { "*" } else { tok };
    let wrapped;
    let tok = if tok.contains(['*', '?']) {
        tok
    } else {
        wrapped = format!("*{tok}*");
        &wrapped
    };
    let mut out = String::from("^");
    for c in tok.chars() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            c => out.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
        }
    }
    out.push('$');
    out
}

fn alternation(masks: &[String], case: bool) -> Result<Option<Regex>, String> {
    if masks.is_empty() {
        return Ok(None);
    }
    let pat = masks.iter().map(|m| wildcard(m)).collect::<Vec<_>>().join("|");
    RegexBuilder::new(&pat)
        .case_insensitive(!case)
        .build()
        .map(Some)
        .map_err(|e| format!("Неверная маска: {e}"))
}

impl NameMatcher {
    fn new(mask: &str, regex: bool, case: bool) -> Result<NameMatcher, String> {
        let mask = mask.trim();
        if regex {
            let include = if mask.is_empty() {
                None
            } else {
                Some(
                    RegexBuilder::new(mask)
                        .case_insensitive(!case)
                        .size_limit(1 << 22)
                        .build()
                        .map_err(|e| regex_error("имени", &e))?,
                )
            };
            return Ok(NameMatcher { include, exclude_files: None, exclude_dirs: None });
        }
        let (inc, exc) = mask.split_once('|').unwrap_or((mask, ""));
        let inc: Vec<String> = tokens(inc).into_iter().filter(|t| t != "*" && t != "*.*").collect();
        let (mut ex_dirs, mut ex_files) = (Vec::new(), Vec::new());
        for t in tokens(exc) {
            match t.strip_suffix(['\\', '/']) {
                Some(d) if !d.is_empty() => ex_dirs.push(d.to_string()),
                Some(_) => {}
                None => ex_files.push(t),
            }
        }
        Ok(NameMatcher {
            include: alternation(&inc, case)?,
            exclude_files: alternation(&ex_files, case)?,
            exclude_dirs: alternation(&ex_dirs, case)?,
        })
    }

    fn matches(&self, name: &str) -> bool {
        self.include.as_ref().map_or(true, |r| r.is_match(name))
    }

    fn file_excluded(&self, name: &str) -> bool {
        self.exclude_files.as_ref().is_some_and(|r| r.is_match(name))
    }

    fn dir_excluded(&self, name: &str) -> bool {
        self.exclude_dirs.as_ref().is_some_and(|r| r.is_match(name))
    }
}

fn regex_error(what: &str, e: &regex::Error) -> String {
    match e {
        regex::Error::Syntax(msg) => {
            let last = msg.lines().rev().find(|l| l.starts_with("error:")).unwrap_or("").trim_start_matches("error:").trim();
            if last.is_empty() {
                format!("Ошибка в регулярном выражении для {what}")
            } else {
                format!("Ошибка в регулярном выражении для {what}: {last}")
            }
        }
        regex::Error::CompiledTooBig(_) => "Слишком сложное регулярное выражение".into(),
        _ => format!("Ошибка в регулярном выражении для {what}"),
    }
}

// ───────────────────────────── text inside files ─────────────────────────────

/// Upper/lower variants of a character (just the character when case matters).
fn variants(c: char, case: bool) -> Vec<char> {
    let mut v = vec![c];
    if !case {
        for o in c.to_lowercase().chain(c.to_uppercase()) {
            if !v.contains(&o) {
                v.push(o);
            }
        }
    }
    v
}

fn cp1251(c: char) -> Option<u8> {
    match c {
        c if c.is_ascii() => Some(c as u8),
        'А'..='я' => Some(0xC0 + (c as u32 - 'А' as u32) as u8),
        'Ё' => Some(0xA8),
        'ё' => Some(0xB8),
        '№' => Some(0xB9),
        _ => None,
    }
}

/// Byte-level pattern for `text` in another encoding; `None` if a character can't be encoded.
fn encoded_branch(text: &str, case: bool, enc: impl Fn(char) -> Option<Vec<u8>>) -> Option<String> {
    let mut out = String::from("(?-u:");
    for c in text.chars() {
        let alts: Option<Vec<String>> = variants(c, case)
            .into_iter()
            .map(|v| enc(v).map(|bytes| bytes.iter().map(|b| format!("\\x{b:02X}")).collect::<String>()))
            .collect();
        out.push_str(&format!("(?:{})", alts?.join("|")));
    }
    out.push(')');
    Some(out)
}

fn text_regex(t: &TextQuery) -> Result<BytesRegex, String> {
    let core = if t.regex { t.text.clone() } else { regex::escape(&t.text) };
    let core = if t.words { format!(r"\b(?:{core})\b") } else { format!("(?:{core})") };
    let mut branches = vec![core];
    if t.other_encodings && !t.regex {
        let utf16 = |c: char| Some(c.encode_utf16(&mut [0; 2]).iter().flat_map(|u| u.to_le_bytes()).collect());
        branches.extend(encoded_branch(&t.text, t.case, utf16));
        if !t.text.is_ascii() {
            branches.extend(encoded_branch(&t.text, t.case, |c| cp1251(c).map(|b| vec![b])));
        }
    }
    BytesRegexBuilder::new(&branches.join("|"))
        .case_insensitive(!t.case)
        .multi_line(true)
        .size_limit(1 << 24)
        .build()
        .map_err(|e| regex_error("текста", &e))
}

const CHUNK: u64 = 1 << 20;
const OVERLAP: usize = 4096;

/// Line around a match, cleaned up for display.
fn snippet(buf: &[u8], start: usize, end: usize) -> String {
    let ls = buf[..start].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1).max(start.saturating_sub(60));
    let le = buf[end..].iter().position(|&b| b == b'\n' || b == b'\r').map_or(buf.len(), |p| end + p).min(end + 140);
    let s: String = String::from_utf8_lossy(&buf[ls..le])
        .chars()
        .map(|c| if c.is_control() || c == '\u{FFFD}' { ' ' } else { c })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let pre = if ls > 0 && buf[ls - 1] != b'\n' { "…" } else { "" };
    format!("{pre}{s}")
}

/// Stream the file in chunks (with overlap so matches across a boundary are found).
/// `Ok(Some(line))` when the text is present.
fn find_in_file(path: &Path, re: &BytesRegex, cancel: &AtomicBool) -> io::Result<Option<String>> {
    let mut f = File::open(path)?;
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK as usize + OVERLAP);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let keep = buf.len().min(OVERLAP);
        buf.drain(..buf.len() - keep);
        let n = (&mut f).take(CHUNK).read_to_end(&mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        if let Some(m) = re.find(&buf) {
            return Ok(Some(snippet(&buf, m.start(), m.end())));
        }
    }
}

// ───────────────────────────── worker ─────────────────────────────

struct Worker {
    q: Query,
    names: NameMatcher,
    text: Option<(BytesRegex, bool)>,
    tx: Sender<Msg>,
    ctx: egui::Context,
    cancel: Arc<AtomicBool>,
    batch: Vec<Hit>,
    last: Instant,
    files: u64,
    dirs: u64,
    errors: u64,
    current: String,
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

impl Worker {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn flush(&mut self, force: bool) {
        if force || self.last.elapsed() > Duration::from_millis(60) {
            if !self.batch.is_empty() {
                let _ = self.tx.send(Msg::Found(std::mem::take(&mut self.batch)));
            }
            let _ = self.tx.send(Msg::Progress { current: self.current.clone(), files: self.files, dirs: self.dirs });
            self.ctx.request_repaint();
            self.last = Instant::now();
        }
    }

    fn meta_ok(&self, is_dir: bool, size: u64, modified: Option<SystemTime>) -> bool {
        if let Some((cmp, v, unit)) = self.q.size {
            if is_dir {
                return false;
            }
            let ok = match cmp {
                SizeCmp::Less => size < v.saturating_mul(unit),
                SizeCmp::Greater => size > v.saturating_mul(unit),
                // with KB / MB the size is compared after rounding to that unit
                SizeCmp::Equal => (size + unit / 2) / unit.max(1) == v,
            };
            if !ok {
                return false;
            }
        }
        if let Some(t) = self.q.newer_than {
            if !modified.is_some_and(|m| m >= t) {
                return false;
            }
        }
        if let Some(t) = self.q.older_than {
            if !modified.is_some_and(|m| m <= t) {
                return false;
            }
        }
        true
    }

    fn run(&mut self) {
        let roots = self.q.roots.clone();
        for root in roots {
            if self.cancelled() {
                break;
            }
            self.walk(root);
        }
        self.flush(true);
        let _ = self.tx.send(Msg::Done { errors: self.errors });
        self.ctx.request_repaint();
    }

    fn walk(&mut self, root: PathBuf) {
        let mut stack = vec![(root, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if self.cancelled() {
                return;
            }
            self.dirs += 1;
            self.current = dir.display().to_string();
            self.flush(false);
            let rd = match fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => {
                    self.errors += 1;
                    continue;
                }
            };
            let mut items: Vec<(String, PathBuf, fs::Metadata)> = Vec::new();
            for de in rd.flatten() {
                match de.metadata() {
                    Ok(m) => items.push((de.file_name().to_string_lossy().into_owned(), de.path(), m)),
                    Err(_) => self.errors += 1,
                }
            }
            items.sort_by_cached_key(|(n, _, _)| n.to_lowercase());
            let mut subdirs = Vec::new();
            for (name, path, link_md) in items {
                if self.cancelled() {
                    return;
                }
                if !self.q.hidden && is_hidden(&name, &link_md) {
                    continue;
                }
                let is_link = link_md.file_type().is_symlink();
                let md = if is_link {
                    match fs::metadata(&path) {
                        Ok(m) => m,
                        Err(_) => continue, // broken link
                    }
                } else {
                    link_md
                };
                let modified = md.modified().ok();
                if md.is_dir() {
                    if self.names.dir_excluded(&name) {
                        continue;
                    }
                    if self.q.kinds != Kinds::Files
                        && self.text.is_none()
                        && self.names.matches(&name)
                        && self.meta_ok(true, 0, modified)
                    {
                        self.batch.push(Hit { path: path.clone(), inner: None, is_dir: true, size: 0, modified, snippet: None });
                    }
                    // links and junctions are not followed: they easily form loops
                    if !is_link && self.q.depth.map_or(true, |d| depth < d) {
                        subdirs.push(path);
                    }
                } else {
                    self.files += 1;
                    let size = md.len();
                    if self.q.kinds != Kinds::Dirs
                        && self.names.matches(&name)
                        && !self.names.file_excluded(&name)
                        && self.meta_ok(false, size, modified)
                    {
                        let snippet = match &self.text {
                            None => Some(None),
                            Some((re, not)) => match find_in_file(&path, re, &self.cancel) {
                                _ if self.cancelled() => None,
                                Ok(found) if *not => found.is_none().then_some(None),
                                Ok(found) => found.map(Some),
                                Err(_) => {
                                    self.errors += 1;
                                    None
                                }
                            },
                        };
                        if let Some(snippet) = snippet {
                            self.batch.push(Hit { path: path.clone(), inner: None, is_dir: false, size, modified, snippet });
                        }
                    }
                    if self.q.archives && name.to_ascii_lowercase().ends_with(".zip") {
                        self.search_zip(&path);
                    }
                }
                self.flush(false);
            }
            stack.extend(subdirs.into_iter().rev().map(|p| (p, depth + 1)));
        }
    }

    /// Match names of the entries of a ZIP archive (text inside is not searched).
    fn search_zip(&mut self, archive: &Path) {
        if self.text.is_some() {
            return;
        }
        let Ok(zr) = ZipReader::open(archive) else {
            self.errors += 1;
            return;
        };
        for e in &zr.entries {
            let trimmed = e.name.trim_end_matches('/');
            let mut parts: Vec<&str> = trimmed.split(['/', '\\']).collect();
            let Some(name) = parts.pop() else { continue };
            if parts.iter().any(|d| self.names.dir_excluded(d)) {
                continue;
            }
            if self.q.depth.is_some_and(|d| parts.len() > d) {
                continue;
            }
            let ok = if e.dir {
                self.q.kinds != Kinds::Files && self.names.matches(name)
            } else {
                self.q.kinds != Kinds::Dirs && self.names.matches(name) && !self.names.file_excluded(name)
            };
            let modified = e.modified();
            if ok && self.meta_ok(e.dir, e.size, modified) {
                self.batch.push(Hit {
                    path: archive.to_path_buf(),
                    inner: Some(trimmed.to_string()),
                    is_dir: e.dir,
                    size: e.size,
                    modified,
                    snippet: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(mask: &str) -> NameMatcher {
        NameMatcher::new(mask, false, false).unwrap()
    }

    #[test]
    fn masks() {
        let m = names("*.txt;*.doc");
        assert!(m.matches("a.TXT") && m.matches("b.doc") && !m.matches("c.pdf"));
        let m = names("*.*");
        assert!(m.matches("noext"));
        let m = names("отчёт");
        assert!(m.matches("Годовой ОТЧЁТ 2025.pdf"));
        let m = names("*.log | temp* cache\\ .git/");
        assert!(m.matches("a.log") && m.file_excluded("temp1.log"));
        assert!(m.dir_excluded("Cache") && m.dir_excluded(".git") && !m.dir_excluded("src"));
        let m = names("\"my file*\" *.rs");
        assert!(m.matches("my file 2.txt") && m.matches("main.rs") && !m.matches("my.txt"));
    }

    fn text(t: &str, f: impl FnOnce(&mut TextQuery)) -> BytesRegex {
        let mut q = TextQuery { text: t.into(), case: false, words: false, regex: false, not_containing: false, other_encodings: false };
        f(&mut q);
        text_regex(&q).unwrap()
    }

    #[test]
    fn text_patterns() {
        assert!(text("hello", |_| {}).is_match(b"say HELLO there"));
        assert!(!text("hello", |q| q.case = true).is_match(b"say HELLO there"));
        assert!(!text("cat", |q| q.words = true).is_match(b"concatenate"));
        assert!(text("cat", |q| q.words = true).is_match(b"a cat."));
        assert!(text(r"fn \w+\(", |q| q.regex = true).is_match(b"pub fn main() {}"));
        assert!(text("a.b", |_| {}).is_match(b"xa.by") && !text("a.b", |_| {}).is_match(b"axb"));

        let utf16: Vec<u8> = "Привет".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let re = text("привет", |q| q.other_encodings = true);
        assert!(re.is_match(&utf16));
        assert!(re.is_match(&[0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2])); // "Привет" in Windows-1251
        assert!(re.is_match("ПРИВЕТ".as_bytes()));
        assert!(!text("привет", |_| {}).is_match(&utf16));
    }

    #[test]
    fn walk_and_filters() {
        let dir = std::env::temp_dir().join("quokka-search-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub/deep")).unwrap();
        fs::create_dir_all(dir.join("skip")).unwrap();
        fs::write(dir.join("a.txt"), "needle here").unwrap();
        fs::write(dir.join("b.txt"), "nothing").unwrap();
        fs::write(dir.join("sub/c.txt"), vec![b'x'; 5000]).unwrap();
        fs::write(dir.join("sub/deep/d.txt"), "NEEDLE").unwrap();
        fs::write(dir.join("skip/e.txt"), "needle").unwrap();
        // a match straddling the chunk boundary
        let mut big = vec![b'.'; CHUNK as usize - 3];
        big.extend_from_slice(b"needle");
        fs::write(dir.join("sub/big.bin"), big).unwrap();

        let run = |f: &dyn Fn(&mut Query)| -> Vec<String> {
            let mut q = Query {
                roots: vec![dir.clone()],
                mask: String::new(),
                mask_regex: false,
                mask_case: false,
                depth: None,
                kinds: Kinds::All,
                hidden: true,
                archives: false,
                text: None,
                size: None,
                newer_than: None,
                older_than: None,
            };
            f(&mut q);
            let mut s = Search::start(q, egui::Context::default()).unwrap();
            while s.running {
                std::thread::sleep(Duration::from_millis(5));
                s.poll();
            }
            let mut v: Vec<String> = s.hits.iter().map(|h| h.name()).collect();
            v.sort();
            v
        };
        assert_eq!(run(&|q| q.mask = "*.txt | skip\\".into()), ["a.txt", "b.txt", "c.txt", "d.txt"]);
        assert_eq!(run(&|q| {
            q.mask = "*.txt".into();
            q.depth = Some(0);
        }), ["a.txt", "b.txt"]);
        assert_eq!(run(&|q| q.kinds = Kinds::Dirs), ["deep", "skip", "sub"]);
        assert_eq!(run(&|q| q.size = Some((SizeCmp::Greater, 4, 1024))), ["big.bin", "c.txt"]);
        let needle = TextQuery { text: "needle".into(), case: false, words: false, regex: false, not_containing: false, other_encodings: false };
        let n2 = needle.clone();
        assert_eq!(run(&move |q| q.text = Some(n2.clone())), ["a.txt", "big.bin", "d.txt", "e.txt"]);
        let n3 = TextQuery { not_containing: true, ..needle };
        assert_eq!(run(&move |q| {
            q.mask = "*.txt".into();
            q.text = Some(n3.clone());
        }), ["b.txt", "c.txt"]);
        let _ = fs::remove_dir_all(&dir);
    }
}
