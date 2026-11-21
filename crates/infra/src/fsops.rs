//! Background file operations (copy / move / delete) with progress and cancellation.

use quokka_domain::{tr, trf};
use quokka_core::job::{Job, Reporter};
use quokka_core::ports::FileOps;
use quokka_core::Waker;
use quokka_domain::OpSpec;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn err_msg(e: io::Error, p: &Path) -> String {
    if e.kind() == io::ErrorKind::Interrupted {
        tr!("Отменено", "Cancelled").into()
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
        let suffix = if i == 1 { tr!(" - копия", " - copy").to_string() } else { trf!(" - копия ({i})", " - copy ({i})") };
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
    if crate::archive::is_member(p) { return (crate::archive::size(p).unwrap_or(0), 1); }
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
            return Err(io::Error::new(io::ErrorKind::Interrupted, tr!("Отменено", "Cancelled")));
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
    if crate::archive::is_member(src) { return crate::archive::extract_to(src, dst, overwrite, rep); }
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
    let mut s = format!("{verb}: {}", quokka_domain::format::items_text(n as u64));
    if skipped > 0 {
        s.push_str(&trf!(", пропущено существующих: {skipped}", ", existing skipped: {skipped}"));
    }
    s
}

pub fn start_copy(sources: Vec<PathBuf>, dest: PathBuf, overwrite: bool, waker: Waker) -> Job {
    let title = trf!("Копирование: {}", "Copying: {}", quokka_domain::format::items_text(sources.len() as u64));
    Job::spawn(title, true, waker, move |rep| {
        let pairs = resolve(&sources, &dest)?;
        rep.total = pairs.iter().map(|(s, _)| tree_size(s).0).sum();
        rep.tick(true);
        for (s, t) in &pairs {
            let t = if t == s { unique_copy_name(t) } else { t.clone() };
            if s.is_dir() && t.starts_with(s) {
                return Err(trf!("Нельзя скопировать «{}» в саму себя", "Cannot copy “{}” into itself", name_of(s)));
            }
            copy_rec(s, &t, overwrite, rep).map_err(|e| err_msg(e, s))?;
        }
        Ok(summary(tr!("Скопировано", "Copied"), pairs.len(), rep.skipped))
    })
}

pub fn start_move(sources: Vec<PathBuf>, dest: PathBuf, overwrite: bool, waker: Waker) -> Job {
    let title = trf!("Перемещение: {}", "Moving: {}", quokka_domain::format::items_text(sources.len() as u64));
    Job::spawn(title, false, waker, move |rep| {
        let pairs = resolve(&sources, &dest)?;
        move_pairs(&pairs, overwrite, rep)
    })
}

/// Undo of a move: each `(current, original)` goes back to `original`. Items whose
/// original place is taken again are skipped, nothing is overwritten.
pub fn start_move_back(pairs: Vec<(PathBuf, PathBuf)>, waker: Waker) -> Job {
    let title = trf!("Отмена перемещения: {}", "Undoing move: {}", quokka_domain::format::items_text(pairs.len() as u64));
    Job::spawn(title, false, waker, move |rep| {
        for (_, original) in &pairs {
            if let Some(parent) = original.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).map_err(|e| err_msg(e, parent))?;
                }
            }
        }
        move_pairs(&pairs, false, rep)
    })
}

/// Move every `(source, target)`; records in `rep.moved` the items that landed at a
/// fresh target (not merged into an existing folder), so they can be moved back.
fn move_pairs(pairs: &[(PathBuf, PathBuf)], overwrite: bool, rep: &mut Reporter) -> Result<String, String> {
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
        if s.symlink_metadata().is_err() {
            // gone since (e.g. undo of a file that was deleted meanwhile)
            rep.skipped += 1;
            continue;
        }
        if s.is_dir() && t.starts_with(s) {
            return Err(trf!("Нельзя переместить «{}» в саму себя", "Cannot move “{}” into itself", name_of(s)));
        }
        let mut merged = false;
        if t.exists() {
            if !overwrite {
                rep.skipped += 1;
                continue;
            }
            if t.is_dir() && s.is_dir() {
                merged = true;
            } else if t.is_dir() {
                fs::remove_dir_all(t).map_err(|e| err_msg(e, t))?;
            } else {
                remove_file_force(t).map_err(|e| err_msg(e, t))?;
            }
        }
        let renamed = !t.exists() && fs::rename(s, t).is_ok();
        if !renamed {
            // different volume or merging into an existing folder
            let copied = copy_rec(s, t, overwrite, rep).map_err(|e| err_msg(e, s));
            if copied.is_err() && !merged {
                // a half-copied target must not be taken for the moved item
                let _ = if t.is_dir() { fs::remove_dir_all(t) } else { fs::remove_file(t) };
            }
            copied?;
            if !merged {
                rep.moved.push((s.clone(), t.clone()));
            }
            remove_rec(s, rep).map_err(|e| err_msg(e, s))?;
            rep.done = i as u64 + 1;
        } else {
            rep.moved.push((s.clone(), t.clone()));
        }
        moved += 1;
    }
    rep.done = rep.total;
    rep.tick(true);
    Ok(summary(tr!("Перемещено", "Moved"), moved, rep.skipped))
}

pub fn start_delete(sources: Vec<PathBuf>, waker: Waker) -> Job {
    let title = trf!("Удаление: {}", "Deleting: {}", quokka_domain::format::items_text(sources.len() as u64));
    Job::spawn(title, false, waker, move |rep| {
        rep.total = sources.iter().map(|s| tree_size(s).1).sum();
        rep.tick(true);
        for s in &sources {
            remove_rec(s, rep).map_err(|e| err_msg(e, s))?;
        }
        Ok(summary(tr!("Удалено", "Deleted"), sources.len(), 0))
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
                Err(io::Error::new(io::ErrorKind::Interrupted, tr!("Отменено", "Cancelled")))
            } else {
                Ok(())
            }
        })
    }
}

/// Pack `sources` into a new ZIP file at `dest`.
pub fn start_zip(sources: Vec<PathBuf>, dest: PathBuf, waker: Waker) -> Job {
    let title = trf!("Архивация: {}", "Zipping: {}", quokka_domain::format::items_text(sources.len() as u64));
    Job::spawn(title, true, waker, move |rep| {
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
            return Err(if e.kind() == io::ErrorKind::Interrupted { tr!("Отменено", "Cancelled").into() } else { format!("{}: {}", name_of(&dest), e) });
        }
        let size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        Ok(trf!("Создан архив «{}» ({})", "Archive “{}” created ({})", name_of(&dest), quokka_domain::format::format_size(size)))
    })
}

/// Extract every entry of `archive` into the folder `dest` (created if missing).
pub fn start_unzip(archive: PathBuf, dest: PathBuf, overwrite: bool, waker: Waker) -> Job {
    let title = trf!("Распаковка: {}", "Extracting: {}", name_of(&archive));
    Job::spawn(title, true, waker, move |rep| {
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
                        Err(io::Error::new(io::ErrorKind::Interrupted, tr!("Отменено", "Cancelled")))
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
        let mut s = trf!("Распаковано из «{}»: {}", "Extracted from “{}”: {}", name_of(&archive), quokka_domain::format::items_text(files as u64));
        if rep.skipped > 0 {
            s.push_str(&trf!(", пропущено существующих: {}", ", existing skipped: {}", rep.skipped));
        }
        if unsafe_names > 0 {
            s.push_str(&trf!(", пропущено небезопасных путей: {unsafe_names}", ", unsafe paths skipped: {unsafe_names}"));
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
        return Err(io::Error::new(io::ErrorKind::Other, trf!("не удалось переместить в корзину (код {rc:#x})", "failed to move to Recycle Bin (code {rc:#x})")));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err(io::Error::new(io::ErrorKind::Interrupted, tr!("Отменено", "Cancelled")));
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
    Err(io::Error::new(io::ErrorKind::Unsupported, tr!("корзина доступна только в Windows", "Recycle Bin is only available on Windows")))
}

/// Whether "delete" can go to the Recycle Bin on this platform.
pub const HAS_RECYCLE_BIN: bool = cfg!(windows);

pub fn start_recycle(sources: Vec<PathBuf>, waker: Waker) -> Job {
    let title = trf!("Перемещение в корзину: {}", "Moving to Recycle Bin: {}", quokka_domain::format::items_text(sources.len() as u64));
    Job::spawn(title, false, waker, move |rep| {
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
        Ok(summary(tr!("Перемещено в корзину", "Moved to Recycle Bin"), sources.len(), 0))
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
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, tr!("на этом месте уже есть файл с таким именем", "a file with this name already exists at this location")));
    }
    let root = target_path
        .ancestors()
        .last()
        .map(Path::to_path_buf)
        .filter(|r| !r.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, tr!("не удалось определить диск", "failed to determine the drive")))?;
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
    let (_, index) = best.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, tr!("не найдено в корзине", "not found in Recycle Bin")))?;
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
    Err(io::Error::new(io::ErrorKind::Unsupported, tr!("корзина доступна только в Windows", "Recycle Bin is only available on Windows")))
}

/// Restore items previously sent to the Recycle Bin by [`start_recycle`].
pub fn start_unrecycle(originals: Vec<PathBuf>, waker: Waker) -> Job {
    let title = trf!("Восстановление из корзины: {}", "Restoring from Recycle Bin: {}", quokka_domain::format::items_text(originals.len() as u64));
    Job::spawn(title, false, waker, move |rep| {
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
            None => Ok(summary(tr!("Восстановлено", "Restored"), restored, 0)),
            Some(first) if errors.len() == 1 && restored == 0 => Err(trf!("Не удалось восстановить {first}", "Failed to restore {first}")),
            Some(first) => Err(trf!(
                "Восстановлено: {}, не удалось: {} ({first})", "Restored: {}, failed: {} ({first})",
                quokka_domain::format::items_text(restored as u64),
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
        tr!("$VISUAL / $EDITOR или системная программа", "$VISUAL / $EDITOR or the system program")
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
            return Err(io::Error::new(io::ErrorKind::InvalidInput, tr!("не задана программа", "no program specified")));
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
            io::Error::new(e.kind(), trf!("программа «{program}» не найдена", "program “{program}” not found"))
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

/// The [`FileOps`] adapter: runs each operation of the queue on its own thread.
pub struct StdFileOps;

impl FileOps for StdFileOps {
    fn start(&self, spec: &OpSpec, waker: Waker) -> Job {
        if let Err(error) = validate_operation(spec) {
            return Job::spawn(spec.title(), false, waker, move |_| Err(error));
        }
        if let Some(job) = start_remote(spec, waker.clone()) {
            return job;
        }
        match spec.clone() {
            OpSpec::Copy { sources, dest, overwrite } => start_copy(sources, dest, overwrite, waker),
            OpSpec::Move { sources, dest, overwrite } => start_move(sources, dest, overwrite, waker),
            OpSpec::Delete { sources } => start_delete(sources, waker),
            OpSpec::Recycle { sources } => start_recycle(sources, waker),
            OpSpec::Restore { originals } => start_unrecycle(originals, waker),
            OpSpec::MoveBack { pairs } => start_move_back(pairs, waker),
            OpSpec::Zip { sources, dest } => start_zip(sources, dest, waker),
            OpSpec::Unzip { archive, dest, overwrite } => start_unzip(archive, dest, overwrite, waker),
        }
    }
}

/// Operations that touch an SFTP server run through [`crate::xfer`].
fn start_remote(spec: &OpSpec, waker: Waker) -> Option<Job> {
    use crate::xfer;
    let title = spec.title();
    Some(match spec.clone() {
        OpSpec::Copy { sources, dest, overwrite } if xfer::involves_remote(&sources, Some(&dest)) => {
            Job::spawn(title, true, waker, move |rep| xfer::copy(&sources, &dest, overwrite, rep))
        }
        OpSpec::Move { sources, dest, overwrite } if xfer::involves_remote(&sources, Some(&dest)) => {
            Job::spawn(title, false, waker, move |rep| xfer::move_items(&sources, &dest, overwrite, rep))
        }
        // a server has no Recycle Bin: deleting there is always permanent
        OpSpec::Delete { sources } | OpSpec::Recycle { sources } if xfer::involves_remote(&sources, None) => {
            Job::spawn(title, false, waker, move |rep| xfer::delete(&sources, rep))
        }
        OpSpec::MoveBack { pairs } if pairs.iter().any(|(a, b)| xfer::involves_remote(&[a.clone(), b.clone()], None)) => {
            Job::spawn(title, false, waker, move |rep| {
                for (from, to) in &pairs {
                    let parent = quokka_domain::paths::parent_of(to).unwrap_or_default();
                    if !parent.as_os_str().is_empty() {
                        crate::xfer::mkdir_all(&parent).map_err(|e| e.to_string())?;
                    }
                    crate::xfer::move_items(std::slice::from_ref(from), to, false, rep)?;
                }
                Ok(tr!("Перемещение отменено", "Move undone").into())
            })
        }
        _ => return None,
    })
}

/// The real queue with the real file operations (moved here from the core tests,
/// which use a fake instead).
#[cfg(test)]
mod queue_tests {
    use super::*;
    use quokka_core::queue::{Finished, Queue};

    fn wait(q: &mut Queue) -> Vec<Finished> {
        let mut all = Vec::new();
        for _ in 0..500 {
            all.extend(q.poll(&StdFileOps, &Waker::noop()));
            if !q.is_active() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        all
    }

    #[test]
    fn zip_unzip_delete_in_order() {
        let dir = std::env::temp_dir().join("quokka-queue-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.txt"), "данные".repeat(1000)).unwrap();

        let mut q = Queue::default();
        let zip = dir.join("a.zip");
        q.push(OpSpec::Zip { sources: vec![dir.join("src")], dest: zip.clone() });
        q.push(OpSpec::Unzip { archive: zip.clone(), dest: dir.join("out"), overwrite: false });
        q.push(OpSpec::Delete { sources: vec![zip.clone()] });
        let done = wait(&mut q);

        assert_eq!(done.len(), 3, "all tasks finish");
        assert!(done.iter().all(|f| f.result.is_ok()), "{:?}", done.iter().map(|f| f.result.clone()).collect::<Vec<_>>());
        assert_eq!(fs::read_to_string(dir.join("out/src/a.txt")).unwrap(), "данные".repeat(1000));
        assert!(!zip.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_then_move_back() {
        let dir = std::env::temp_dir().join("quokka-move-undo-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/папка")).unwrap();
        fs::create_dir_all(dir.join("dst")).unwrap();
        fs::write(dir.join("src/a.txt"), b"a").unwrap();
        fs::write(dir.join("src/папка/b.txt"), b"b").unwrap();

        let mut q = Queue::default();
        let sources = vec![dir.join("src/a.txt"), dir.join("src/папка")];
        q.push(OpSpec::Move { sources, dest: dir.join("dst"), overwrite: false });
        let done = wait(&mut q);
        assert!(done[0].result.is_ok(), "{:?}", done[0].result);
        assert_eq!(done[0].moved.len(), 2);
        assert!(dir.join("dst/папка/b.txt").exists() && !dir.join("src/a.txt").exists());

        let back = done[0].moved.iter().map(|(from, to)| (to.clone(), from.clone())).collect();
        q.push(OpSpec::MoveBack { pairs: back });
        let done = wait(&mut q);
        assert!(done[0].result.is_ok(), "{:?}", done[0].result);
        assert_eq!(fs::read(dir.join("src/a.txt")).unwrap(), b"a");
        assert_eq!(fs::read(dir.join("src/папка/b.txt")).unwrap(), b"b");
        assert!(fs::read_dir(dir.join("dst")).unwrap().next().is_none());
        let _ = fs::remove_dir_all(&dir);
    }
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

}

/// ZIP folders are read-only; only Copy may read virtual members.
pub fn validate_operation(spec: &OpSpec) -> Result<(), String> {
    let readonly = || Err(tr!("Архив открыт только для чтения. F5 — извлечь выбранное в обычную папку.", "Archive is read-only. F5 extracts selected items to a regular folder.").into());
    let (sources, dest, allow_members): (&[PathBuf], Option<&Path>, bool) = match spec {
        OpSpec::Copy { sources, dest, .. } => (sources, Some(dest), true),
        OpSpec::Move { sources, dest, .. } | OpSpec::Zip { sources, dest } => (sources, Some(dest), false),
        OpSpec::Delete { sources } | OpSpec::Recycle { sources } => (sources, None, false),
        OpSpec::Unzip { archive, dest, .. } => (std::slice::from_ref(archive), Some(dest), false),
        _ => return Ok(()),
    };
    if matches!(spec, OpSpec::Zip { .. } | OpSpec::Unzip { .. }) && crate::xfer::involves_remote(sources, dest) {
        return Err(tr!("Архивы на сервере не поддерживаются — сначала скопируйте файлы на диск.", "Archives on a server are not supported — copy the files to a local disk first.").into());
    }
    if dest.is_some_and(|d| crate::archive::location(d).is_some()) { return readonly(); }
    if !allow_members && sources.iter().any(|s| crate::archive::is_member(s)) { return readonly(); }
    Ok(())
}

/// Launch an interactive terminal using an argument list; folder names are never
/// interpolated into shell source code.
pub fn open_terminal(path: &Path) -> io::Result<()> {
    let archive = crate::archive::location(path);
    let folder = archive.as_ref().and_then(|(a, _)| a.parent()).unwrap_or(path);
    if !folder.is_dir() { return Err(io::Error::new(io::ErrorKind::NotADirectory, tr!("Сначала откройте папку", "Open a folder first"))); }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_CONSOLE gives this explicitly requested interactive terminal
        // its own visible window (Quokka is a GUI executable).
        std::process::Command::new("powershell.exe").args(["-NoLogo", "-NoExit"])
            .current_dir(folder).creation_flags(0x00000010).spawn()?;
    }
    #[cfg(target_os = "macos")]
    { std::process::Command::new("open").args(["-a", "Terminal"]).arg(folder).spawn()?; }
    #[cfg(all(unix, not(target_os = "macos")))]
    { std::process::Command::new("x-terminal-emulator").current_dir(folder).spawn()?; }
    Ok(())
}
