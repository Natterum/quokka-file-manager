//! Background file operations (copy / move / delete) with progress and cancellation.

use eframe::egui;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

pub enum Msg {
    Progress { done: u64, total: u64, current: String },
    Done(Result<String, String>),
}

pub struct Job {
    #[allow(dead_code)] // the queue shows its own, more detailed titles
    pub title: String,
    pub bytes: bool,
    rx: Receiver<Msg>,
    pub cancel: Arc<AtomicBool>,
    pub done: u64,
    pub total: u64,
    pub current: String,
    pub finished: Option<Result<String, String>>,
}

impl Job {
    pub fn poll(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(Msg::Progress { done, total, current }) => {
                    self.done = done;
                    self.total = total;
                    self.current = current;
                }
                Ok(Msg::Done(r)) => {
                    self.finished = Some(r);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.finished.is_none() {
                        self.finished = Some(Err("Операция прервана".into()));
                    }
                    break;
                }
            }
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f64 / self.total as f64).clamp(0.0, 1.0) as f32
        }
    }
}

struct Reporter {
    tx: Sender<Msg>,
    ctx: egui::Context,
    cancel: Arc<AtomicBool>,
    done: u64,
    total: u64,
    current: String,
    last: Instant,
    count_bytes: bool,
    skipped: usize,
}

impl Reporter {
    fn tick(&mut self, force: bool) {
        if force || self.last.elapsed().as_millis() > 50 {
            let _ = self.tx.send(Msg::Progress { done: self.done, total: self.total, current: self.current.clone() });
            self.ctx.request_repaint();
            self.last = Instant::now();
        }
    }

    fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(io::ErrorKind::Interrupted, "Отменено"))
        } else {
            Ok(())
        }
    }
}

fn spawn<F>(title: String, bytes: bool, ctx: egui::Context, f: F) -> Job
where
    F: FnOnce(&mut Reporter) -> Result<String, String> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut rep = Reporter {
        tx,
        ctx,
        cancel: cancel.clone(),
        done: 0,
        total: 0,
        current: String::new(),
        last: Instant::now(),
        count_bytes: bytes,
        skipped: 0,
    };
    // shell operations (Recycle Bin, shell extensions such as OneDrive or antivirus
    // hooks) can need far more stack than Rust's 2 MB default for spawned threads
    let spawned = thread::Builder::new().name("quokka-job".into()).stack_size(32 * 1024 * 1024).spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut rep)))
            .unwrap_or_else(|e| Err(format!("Внутренняя ошибка: {}", panic_text(&e))));
        let _ = rep.tx.send(Msg::Done(r));
        rep.ctx.request_repaint();
    });
    if let Err(e) = spawned {
        let (tx2, rx2) = mpsc::channel();
        let _ = tx2.send(Msg::Done(Err(format!("Не удалось запустить операцию: {e}"))));
        return Job { title, bytes, rx: rx2, cancel, done: 0, total: 0, current: String::new(), finished: None };
    }
    Job { title, bytes, rx, cancel, done: 0, total: 0, current: String::new(), finished: None }
}

fn panic_text(e: &Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "неизвестная ошибка".into())
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn err_msg(e: io::Error, p: &Path) -> String {
    if e.kind() == io::ErrorKind::Interrupted {
        "Отменено".into()
    } else {
        format!("{}: {}", name_of(p), e)
    }
}

/// Pairs (source, target). A multi-item destination that does not exist is created as a folder.
fn resolve(sources: &[PathBuf], dest: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    if sources.len() > 1 && !dest.is_dir() {
        fs::create_dir_all(dest).map_err(|e| format!("{}: {}", dest.display(), e))?;
    }
    let dest_dir = dest.is_dir();
    Ok(sources
        .iter()
        .map(|s| {
            let t = if dest_dir { dest.join(s.file_name().unwrap_or_default()) } else { dest.to_path_buf() };
            (s.clone(), t)
        })
        .collect())
}

fn unique_copy_name(p: &Path) -> PathBuf {
    let parent = p.parent().unwrap_or(Path::new(""));
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let is_dir = p.is_dir();
    for i in 1.. {
        let suffix = if i == 1 { " - копия".to_string() } else { format!(" - копия ({i})") };
        let cand = if is_dir {
            parent.join(format!("{}{}", name_of(p), suffix))
        } else {
            parent.join(format!("{stem}{suffix}{ext}"))
        };
        if !cand.exists() {
            return cand;
        }
    }
    unreachable!()
}

fn tree_size(p: &Path) -> (u64, u64) {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            let mut acc = (0, 1);
            if let Ok(rd) = fs::read_dir(p) {
                for e in rd.flatten() {
                    let (b, c) = tree_size(&e.path());
                    acc.0 += b;
                    acc.1 += c;
                }
            }
            acc
        }
        Ok(m) => (m.len(), 1),
        Err(_) => (0, 1),
    }
}

fn copy_file(src: &Path, dst: &Path, len: u64, rep: &mut Reporter) -> io::Result<()> {
    if len < 8 * 1024 * 1024 {
        fs::copy(src, dst)?;
        if rep.count_bytes {
            rep.done += len;
        }
        rep.tick(false);
        return Ok(());
    }
    let mut r = File::open(src)?;
    let mut w = File::create(dst)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if rep.check().is_err() {
            drop(w);
            let _ = fs::remove_file(dst);
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Отменено"));
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        if rep.count_bytes {
            rep.done += n as u64;
        }
        rep.tick(false);
    }
    w.flush()?;
    if let Ok(m) = fs::metadata(src) {
        let _ = fs::set_permissions(dst, m.permissions());
    }
    Ok(())
}

fn copy_rec(src: &Path, dst: &Path, overwrite: bool, rep: &mut Reporter) -> io::Result<()> {
    rep.check()?;
    let m = fs::metadata(src)?;
    if m.is_dir() {
        fs::create_dir_all(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_rec(&e.path(), &dst.join(e.file_name()), overwrite, rep)?;
        }
        Ok(())
    } else {
        rep.current = name_of(src);
        if dst.exists() && !overwrite {
            rep.skipped += 1;
            if rep.count_bytes {
                rep.done += m.len();
            }
            rep.tick(false);
            return Ok(());
        }
        rep.tick(false);
        copy_file(src, dst, m.len(), rep)
    }
}

fn remove_file_force(p: &Path) -> io::Result<()> {
    match fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) => {
            if let Ok(m) = fs::metadata(p) {
                let mut perm = m.permissions();
                if perm.readonly() {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perm.set_readonly(false);
                    fs::set_permissions(p, perm)?;
                    return fs::remove_file(p);
                }
            }
            Err(e)
        }
    }
}

fn remove_rec(p: &Path, rep: &mut Reporter) -> io::Result<()> {
    rep.check()?;
    let m = fs::symlink_metadata(p)?;
    if m.is_dir() {
        for e in fs::read_dir(p)? {
            remove_rec(&e?.path(), rep)?;
        }
        fs::remove_dir(p)?;
    } else if m.file_type().is_symlink() && fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false) {
        fs::remove_dir(p).or_else(|_| fs::remove_file(p))?;
    } else {
        remove_file_force(p)?;
    }
    rep.done += 1;
    rep.current = name_of(p);
    rep.tick(false);
    Ok(())
}

fn summary(verb: &str, n: usize, skipped: usize) -> String {
    let mut s = format!("{verb}: {}", crate::util::items_text(n as u64));
    if skipped > 0 {
        s.push_str(&format!(", пропущено существующих: {skipped}"));
    }
    s
}

pub fn start_copy(sources: Vec<PathBuf>, dest: PathBuf, overwrite: bool, ctx: egui::Context) -> Job {
    let title = format!("Копирование: {}", crate::util::items_text(sources.len() as u64));
    spawn(title, true, ctx, move |rep| {
        let pairs = resolve(&sources, &dest)?;
        rep.total = pairs.iter().map(|(s, _)| tree_size(s).0).sum();
        rep.tick(true);
        for (s, t) in &pairs {
            let t = if t == s { unique_copy_name(t) } else { t.clone() };
            if s.is_dir() && t.starts_with(s) {
                return Err(format!("Нельзя скопировать «{}» в саму себя", name_of(s)));
            }
            copy_rec(s, &t, overwrite, rep).map_err(|e| err_msg(e, s))?;
        }
        Ok(summary("Скопировано", pairs.len(), rep.skipped))
    })
}

pub fn start_move(sources: Vec<PathBuf>, dest: PathBuf, overwrite: bool, ctx: egui::Context) -> Job {
    let title = format!("Перемещение: {}", crate::util::items_text(sources.len() as u64));
    spawn(title, false, ctx, move |rep| {
        let pairs = resolve(&sources, &dest)?;
        rep.total = pairs.len() as u64;
        rep.tick(true);
        let mut moved = 0;
        for (i, (s, t)) in pairs.iter().enumerate() {
            rep.check().map_err(|e| err_msg(e, s))?;
            rep.current = name_of(s);
            rep.done = i as u64;
            rep.tick(true);
            if t == s {
                continue;
            }
            if s.is_dir() && t.starts_with(s) {
                return Err(format!("Нельзя переместить «{}» в саму себя", name_of(s)));
            }
            if t.exists() {
                if !overwrite {
                    rep.skipped += 1;
                    continue;
                }
                if !(t.is_dir() && s.is_dir()) {
                    if t.is_dir() {
                        fs::remove_dir_all(t).map_err(|e| err_msg(e, t))?;
                    } else {
                        remove_file_force(t).map_err(|e| err_msg(e, t))?;
                    }
                }
            }
            let renamed = !t.exists() && fs::rename(s, t).is_ok();
            if !renamed {
                // different volume or merging into an existing folder
                copy_rec(s, t, overwrite, rep).map_err(|e| err_msg(e, s))?;
                remove_rec(s, rep).map_err(|e| err_msg(e, s))?;
                rep.done = i as u64 + 1;
            }
            moved += 1;
        }
        rep.done = rep.total;
        rep.tick(true);
        Ok(summary("Перемещено", moved, rep.skipped))
    })
}

pub fn start_delete(sources: Vec<PathBuf>, ctx: egui::Context) -> Job {
    let title = format!("Удаление: {}", crate::util::items_text(sources.len() as u64));
    spawn(title, false, ctx, move |rep| {
        rep.total = sources.iter().map(|s| tree_size(s).1).sum();
        rep.tick(true);
        for s in &sources {
            remove_rec(s, rep).map_err(|e| err_msg(e, s))?;
        }
        Ok(summary("Удалено", sources.len(), 0))
    })
}

fn zip_rec(z: &mut crate::zip::ZipWriter, src: &Path, name: &str, out: &Path, rep: &mut Reporter) -> io::Result<()> {
    rep.check()?;
    let m = fs::metadata(src)?;
    if m.is_dir() {
        z.add_dir(name, m.modified().ok())?;
        let mut children: Vec<_> = fs::read_dir(src)?.flatten().collect();
        children.sort_by_key(|e| e.file_name());
        for e in children {
            let child = e.path();
            if child == out {
                continue; // never pack the archive into itself
            }
            let cname = format!("{}/{}", name, e.file_name().to_string_lossy());
            zip_rec(z, &child, &cname, out, rep)?;
        }
        Ok(())
    } else {
        rep.current = name_of(src);
        rep.tick(false);
        let mut f = File::open(src)?;
        let cancel = rep.cancel.clone();
        z.add_file(name, &mut f, m.modified().ok(), |n| {
            rep.done += n;
            rep.tick(false);
            if cancel.load(Ordering::Relaxed) {
                Err(io::Error::new(io::ErrorKind::Interrupted, "Отменено"))
            } else {
                Ok(())
            }
        })
    }
}

/// Pack `sources` into a new ZIP file at `dest`.
pub fn start_zip(sources: Vec<PathBuf>, dest: PathBuf, ctx: egui::Context) -> Job {
    let title = format!("Архивация: {}", crate::util::items_text(sources.len() as u64));
    spawn(title, true, ctx, move |rep| {
        rep.total = sources.iter().map(|s| tree_size(s).0).sum();
        rep.tick(true);
        let result = (|| -> io::Result<()> {
            let mut z = crate::zip::ZipWriter::create(&dest)?;
            for s in &sources {
                zip_rec(&mut z, s, &name_of(s), &dest, rep)?;
            }
            z.finish()
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&dest);
            return Err(if e.kind() == io::ErrorKind::Interrupted { "Отменено".into() } else { format!("{}: {}", name_of(&dest), e) });
        }
        let size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        Ok(format!("Создан архив «{}» ({})", name_of(&dest), crate::util::format_size(size)))
    })
}

/// Extract every entry of `archive` into the folder `dest` (created if missing).
pub fn start_unzip(archive: PathBuf, dest: PathBuf, overwrite: bool, ctx: egui::Context) -> Job {
    let title = format!("Распаковка: {}", name_of(&archive));
    spawn(title, true, ctx, move |rep| {
        let fail = |e: io::Error| format!("{}: {}", name_of(&archive), e);
        let mut z = crate::zip::ZipReader::open(&archive).map_err(fail)?;
        rep.total = z.entries.iter().map(|e| e.size).sum();
        rep.tick(true);
        fs::create_dir_all(&dest).map_err(|e| err_msg(e, &dest))?;
        let mut files = 0usize;
        let mut unsafe_names = 0usize;
        for i in 0..z.entries.len() {
            rep.check().map_err(|e| err_msg(e, &archive))?;
            let Some(rel) = z.entries[i].safe_path() else {
                unsafe_names += 1;
                continue;
            };
            let target = dest.join(&rel);
            if z.entries[i].dir {
                fs::create_dir_all(&target).map_err(|e| err_msg(e, &target))?;
                continue;
            }
            rep.current = name_of(&target);
            rep.tick(false);
            if target.exists() && !overwrite {
                rep.skipped += 1;
                rep.done += z.entries[i].size;
                continue;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| err_msg(e, parent))?;
            }
            let modified = z.entries[i].modified();
            let result = (|| -> io::Result<()> {
                let mut out = io::BufWriter::new(File::create(&target)?);
                let cancel = rep.cancel.clone();
                z.extract(i, &mut out, |n| {
                    rep.done += n;
                    rep.tick(false);
                    if cancel.load(Ordering::Relaxed) {
                        Err(io::Error::new(io::ErrorKind::Interrupted, "Отменено"))
                    } else {
                        Ok(())
                    }
                })?;
                let f = out.into_inner().map_err(|e| e.into_error())?;
                if let Some(t) = modified {
                    let _ = f.set_modified(t);
                }
                Ok(())
            })();
            if let Err(e) = result {
                let _ = fs::remove_file(&target); // never leave a half-written file behind
                return Err(err_msg(e, &target));
            }
            files += 1;
        }
        rep.done = rep.total;
        rep.tick(true);
        let mut s = format!("Распаковано из «{}»: {}", name_of(&archive), crate::util::items_text(files as u64));
        if rep.skipped > 0 {
            s.push_str(&format!(", пропущено существующих: {}", rep.skipped));
        }
        if unsafe_names > 0 {
            s.push_str(&format!(", пропущено небезопасных путей: {unsafe_names}"));
        }
        Ok(s)
    })
}

/// Move one item to the Windows Recycle Bin.
#[cfg(windows)]
fn recycle_one(p: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW,
    };
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir()?.join(p) };
    // SHFileOperation must run in a single-threaded COM apartment
    com_init();
    // pFrom must be double-NUL terminated
    let shown = abs.display().to_string();
    let plain = shown.strip_prefix(r"\\?\").unwrap_or(&shown);
    let mut from: Vec<u16> = std::ffi::OsStr::new(plain).encode_wide().collect();
    from.push(0);
    from.push(0);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE as _;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as _;
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc != 0 {
        return Err(io::Error::new(io::ErrorKind::Other, format!("не удалось переместить в корзину (код {rc:#x})")));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "Отменено"));
    }
    Ok(())
}

/// Initialise COM (STA) once for the current worker thread.
#[cfg(windows)]
fn com_init() {
    use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    thread_local!(static INIT: () = unsafe {
        // S_FALSE / RPC_E_CHANGED_MODE are fine: COM is already usable on this thread
        CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as _);
    });
    INIT.with(|_| ());
}

#[cfg(not(windows))]
fn recycle_one(_p: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "корзина доступна только в Windows"))
}

/// Whether "delete" can go to the Recycle Bin on this platform.
pub const HAS_RECYCLE_BIN: bool = cfg!(windows);

pub fn start_recycle(sources: Vec<PathBuf>, ctx: egui::Context) -> Job {
    let title = format!("Перемещение в корзину: {}", crate::util::items_text(sources.len() as u64));
    spawn(title, false, ctx, move |rep| {
        rep.total = sources.len() as u64;
        rep.tick(true);
        for (i, s) in sources.iter().enumerate() {
            rep.check().map_err(|e| err_msg(e, s))?;
            rep.current = name_of(s);
            rep.done = i as u64;
            rep.tick(true);
            recycle_one(s).map_err(|e| err_msg(e, s))?;
        }
        rep.done = rep.total;
        rep.tick(true);
        Ok(summary("Перемещено в корзину", sources.len(), 0))
    })
}

/// Absolute path as Windows shell APIs see it (no `\\?\` prefix).
#[cfg(windows)]
fn shell_path(p: &Path) -> io::Result<String> {
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir()?.join(p) };
    let shown = abs.display().to_string();
    Ok(shown.strip_prefix(r"\\?\").unwrap_or(&shown).to_string())
}

/// Parse a Recycle Bin `$I…` index file: (original path, deletion FILETIME).
#[cfg(windows)]
fn parse_recycle_index(data: &[u8]) -> Option<(String, u64)> {
    let u64_at = |o: usize| data.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
    let version = u64_at(0)?;
    let deleted = u64_at(16)?;
    let units: Vec<u16> = match version {
        // Vista … 8.1: fixed MAX_PATH buffer
        1 => data.get(24..24 + 520)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect(),
        // Windows 10+: length-prefixed (in UTF-16 units, including the NUL)
        2 => {
            let len = u32::from_le_bytes(data.get(24..28)?.try_into().ok()?) as usize;
            data.get(28..28 + len * 2)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
        }
        _ => return None,
    };
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    Some((String::from_utf16_lossy(&units[..end]), deleted))
}

/// Put an item recycled from `original` back where it was.
#[cfg(windows)]
fn unrecycle_one(original: &Path) -> io::Result<()> {
    let target = shell_path(original)?;
    let target_path = PathBuf::from(&target);
    if target_path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "на этом месте уже есть файл с таким именем"));
    }
    let root = target_path
        .ancestors()
        .last()
        .map(Path::to_path_buf)
        .filter(|r| !r.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "не удалось определить диск"))?;
    let bin = root.join("$Recycle.Bin");
    let want = target.to_lowercase();
    // newest matching entry wins: the same path may have been deleted several times
    let mut best: Option<(u64, PathBuf)> = None;
    // one subfolder per user SID; folders of other users are not readable and are skipped
    for user in fs::read_dir(&bin).into_iter().flatten().flatten() {
        for e in fs::read_dir(user.path()).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("$I") {
                continue;
            }
            let Ok(data) = fs::read(e.path()) else { continue };
            let Some((orig, deleted)) = parse_recycle_index(&data) else { continue };
            if orig.to_lowercase() == want && best.as_ref().map_or(true, |(t, _)| deleted > *t) {
                best = Some((deleted, e.path()));
            }
        }
    }
    let (_, index) = best.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "не найдено в корзине"))?;
    let data_name = format!("$R{}", &index.file_name().unwrap().to_string_lossy()[2..]);
    let data = index.with_file_name(data_name);
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&data, &target_path)?;
    let _ = fs::remove_file(&index);
    Ok(())
}

#[cfg(not(windows))]
fn unrecycle_one(_p: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "корзина доступна только в Windows"))
}

/// Restore items previously sent to the Recycle Bin by [`start_recycle`].
pub fn start_unrecycle(originals: Vec<PathBuf>, ctx: egui::Context) -> Job {
    let title = format!("Восстановление из корзины: {}", crate::util::items_text(originals.len() as u64));
    spawn(title, false, ctx, move |rep| {
        rep.total = originals.len() as u64;
        rep.tick(true);
        let mut errors = Vec::new();
        for (i, p) in originals.iter().enumerate() {
            rep.check().map_err(|e| err_msg(e, p))?;
            rep.current = name_of(p);
            rep.done = i as u64;
            rep.tick(true);
            if let Err(e) = unrecycle_one(p) {
                errors.push(err_msg(e, p));
            }
        }
        rep.done = rep.total;
        rep.tick(true);
        let restored = originals.len() - errors.len();
        match errors.first() {
            None => Ok(summary("Восстановлено", restored, 0)),
            Some(first) if errors.len() == 1 && restored == 0 => Err(format!("Не удалось восстановить {first}")),
            Some(first) => Err(format!(
                "Восстановлено: {}, не удалось: {} ({first})",
                crate::util::items_text(restored as u64),
                errors.len()
            )),
        }
    })
}

/// Placeholder shown in the empty "external editor" settings field.
pub fn default_editor_hint() -> &'static str {
    if cfg!(windows) {
        "notepad.exe"
    } else {
        "$VISUAL / $EDITOR или системная программа"
    }
}

/// Splits a command line into words; double quotes group words with spaces.
fn split_command(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut has = false;
    for c in cmd.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// Opens `path` in an external text editor.
///
/// `editor` is a program path or a command line (quotes allowed); `%f` in it is
/// replaced by the file path, otherwise the path is appended. An unquoted path
/// with spaces to an existing program also works. Empty `editor` means Notepad
/// on Windows, `$VISUAL`/`$EDITOR` elsewhere, falling back to the system handler.
pub fn open_in_editor(editor: &str, path: &Path) -> io::Result<()> {
    let mut cmd = editor.trim().to_string();
    if cmd.is_empty() {
        if cfg!(windows) {
            cmd = "notepad.exe".into();
        } else if let Some(v) = std::env::var("VISUAL").ok().or_else(|| std::env::var("EDITOR").ok()).filter(|v| !v.trim().is_empty()) {
            cmd = v;
        } else {
            return open_external(path);
        }
    }
    let (program, mut args) = if Path::new(&cmd).is_file() {
        (cmd.clone(), Vec::new())
    } else {
        let mut words = split_command(&cmd);
        if words.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "не задана программа"));
        }
        let program = words.remove(0);
        (program, words)
    };
    let file = path.as_os_str().to_string_lossy().into_owned();
    let mut used = false;
    for a in args.iter_mut() {
        if a.contains("%f") {
            *a = a.replace("%f", &file);
            used = true;
        }
    }
    let mut command = std::process::Command::new(&program);
    command.args(&args);
    if !used {
        command.arg(path);
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        command.current_dir(dir);
    }
    command.spawn().map(|_| ()).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            io::Error::new(e.kind(), format!("программа «{program}» не найдена"))
        } else {
            e
        }
    })
}

#[cfg(test)]
mod editor_tests {
    use super::split_command;

    #[test]
    fn splits_quoted_command() {
        assert_eq!(
            split_command(r#""C:\Program Files\Notepad++\notepad++.exe" -multiInst "%f""#),
            vec![r"C:\Program Files\Notepad++\notepad++.exe", "-multiInst", "%f"]
        );
        assert_eq!(split_command("code  -w"), vec!["code", "-w"]);
        assert!(split_command("   ").is_empty());
    }
}

pub fn open_external(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer").arg(path).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn recycle_from_worker_thread() {
        let dir = std::env::temp_dir().join("quokka-recycle-test");
        let _ = fs::create_dir_all(&dir);
        let f = dir.join("удалить меня.txt");
        fs::write(&f, b"x").unwrap();
        let f2 = f.clone();
        let r = std::thread::spawn(move || recycle_one(&f2)).join();
        assert!(r.is_ok(), "thread panicked");
        r.unwrap().unwrap();
        assert!(!f.exists());
    }

    #[test]
    fn recycle_then_restore() {
        let dir = std::env::temp_dir().join("quokka-unrecycle-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("папка")).unwrap();
        let f = dir.join("вернуть меня.txt");
        fs::write(&f, b"hello").unwrap();
        fs::write(dir.join("папка").join("a.txt"), b"a").unwrap();
        let sub = dir.join("папка");
        recycle_one(&f).unwrap();
        recycle_one(&sub).unwrap();
        assert!(!f.exists() && !sub.exists());
        unrecycle_one(&f).unwrap();
        unrecycle_one(&sub).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"hello");
        assert_eq!(fs::read(sub.join("a.txt")).unwrap(), b"a");
        let _ = fs::remove_dir_all(&dir);
    }
}
