//! Copy / move / delete when at least one side is a server (SFTP / FTP): download, upload,
//! server-to-server and deletes on the server. The local-only paths stay in
//! [`crate::fsops`], which hands over here as soon as a remote path is involved.

use crate::net::{self, Node};
use quokka_core::job::Reporter;
use quokka_domain::remote;
use quokka_domain::{tr, trf};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// One side of a transfer.
enum Loc {
    Local(PathBuf),
    Remote(Node),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dir,
    File(u64),
    Link,
}

fn loc(p: &Path) -> io::Result<Loc> {
    if remote::is_remote(p) {
        Ok(Loc::Remote(net::resolve(p)?))
    } else {
        Ok(Loc::Local(p.to_path_buf()))
    }
}

pub fn involves_remote(paths: &[PathBuf], dest: Option<&Path>) -> bool {
    paths.iter().any(|p| remote::is_remote(p)) || dest.is_some_and(remote::is_remote)
}

fn name_of(p: &Path) -> String {
    remote::file_name(p).unwrap_or_else(|| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

fn join(dir: &Path, name: &str) -> PathBuf {
    remote::join(dir, name).unwrap_or_else(|| dir.join(name))
}

fn err_msg(e: io::Error, p: &Path) -> String {
    if e.kind() == io::ErrorKind::Interrupted {
        tr!("Отменено", "Cancelled").into()
    } else {
        format!("{}: {}", remote::display(p).map(|_| name_of(p)).unwrap_or_else(|| name_of(p)), e)
    }
}

impl Loc {
    fn kind(&self) -> io::Result<Option<Kind>> {
        match self {
            Loc::Local(p) => match fs::symlink_metadata(p) {
                Ok(m) if m.file_type().is_symlink() => match fs::metadata(p) {
                    Ok(t) if t.is_dir() => Ok(Some(Kind::Link)),
                    Ok(t) => Ok(Some(Kind::File(t.len()))),
                    Err(_) => Ok(Some(Kind::Link)),
                },
                Ok(m) if m.is_dir() => Ok(Some(Kind::Dir)),
                Ok(m) => Ok(Some(Kind::File(m.len()))),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            },
            Loc::Remote(r) => Ok(r.lstat()?.map(|s| match (s.is_link, s.is_dir) {
                (true, true) => Kind::Link,
                (false, true) => Kind::Dir,
                _ => Kind::File(s.len),
            })),
        }
    }

    fn exists(&self) -> bool {
        self.kind().ok().flatten().is_some()
    }

    fn is_dir(&self) -> bool {
        matches!(self.kind(), Ok(Some(Kind::Dir)))
    }

    fn names(&self) -> io::Result<Vec<String>> {
        match self {
            Loc::Local(p) => fs::read_dir(p)?.map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned())).collect(),
            Loc::Remote(r) => Ok(r.list(&AtomicBool::new(false))?.into_iter().map(|(n, _)| n).collect()),
        }
    }

    fn child(&self, name: &str) -> Loc {
        match self {
            Loc::Local(p) => Loc::Local(p.join(name)),
            Loc::Remote(r) => Loc::Remote(r.child(name)),
        }
    }

    fn mkdir_all(&self) -> io::Result<()> {
        match self {
            Loc::Local(p) => fs::create_dir_all(p),
            Loc::Remote(r) => r.mkdir_all(),
        }
    }

    fn remove_file(&self) -> io::Result<()> {
        match self {
            Loc::Local(p) => fs::remove_file(p),
            Loc::Remote(r) => r.remove_file(),
        }
    }

    fn remove_dir(&self) -> io::Result<()> {
        match self {
            Loc::Local(p) => fs::remove_dir(p),
            Loc::Remote(r) => r.remove_dir(),
        }
    }

    fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        match self {
            Loc::Local(p) => Ok(Box::new(fs::File::open(p)?)),
            Loc::Remote(r) => r.reader(),
        }
    }

    fn label(&self) -> PathBuf {
        match self {
            Loc::Local(p) => p.clone(),
            Loc::Remote(r) => r.local_path(),
        }
    }
}

/// Total bytes and item count below `l`, links not followed.
fn tree_size(l: &Loc, rep: &Reporter) -> io::Result<(u64, u64)> {
    rep.check()?;
    match l.kind()? {
        Some(Kind::Dir) => {
            let mut acc = (0, 1);
            for n in l.names()? {
                let (b, c) = tree_size(&l.child(&n), rep)?;
                acc = (acc.0 + b, acc.1 + c);
            }
            Ok(acc)
        }
        Some(Kind::File(len)) => Ok((len, 1)),
        Some(Kind::Link) => Ok((0, 1)),
        None => Ok((0, 0)),
    }
}

fn copy_file(src: &Loc, dst: &Loc, rep: &mut Reporter) -> io::Result<()> {
    let mut r = src.reader()?;
    let result = (|| -> io::Result<()> {
        let mut buf = vec![0u8; 256 * 1024];
        match dst {
            Loc::Local(p) => {
                let mut w = fs::File::create(p)?;
                pump(&mut *r, &mut w, &mut buf, rep)?;
                w.flush()
            }
            Loc::Remote(t) => {
                let mut w = t.writer()?;
                pump(&mut *r, &mut *w, &mut buf, rep)?;
                w.finish()
            }
        }
    })();
    if result.is_err() {
        // never leave a truncated copy that looks complete
        let _ = dst.remove_file();
    }
    result
}

fn pump(r: &mut dyn Read, w: &mut dyn Write, buf: &mut [u8], rep: &mut Reporter) -> io::Result<()> {
    loop {
        rep.check()?;
        let n = match r.read(buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        w.write_all(&buf[..n])?;
        if rep.count_bytes {
            rep.done += n as u64;
        }
        rep.tick(false);
    }
}

fn copy_rec(src: &Loc, dst: &Loc, overwrite: bool, rep: &mut Reporter) -> io::Result<()> {
    rep.check()?;
    match src.kind()?.ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))? {
        Kind::Dir => {
            if !dst.is_dir() {
                dst.mkdir_all()?;
            }
            for n in src.names()? {
                copy_rec(&src.child(&n), &dst.child(&n), overwrite, rep)?;
            }
            Ok(())
        }
        // a link to a folder: copying through it could loop forever
        Kind::Link => {
            rep.skipped += 1;
            Ok(())
        }
        Kind::File(len) => {
            rep.current = name_of(&src.label());
            if dst.exists() && (!overwrite || dst.is_dir()) {
                rep.skipped += 1;
                if rep.count_bytes {
                    rep.done += len;
                }
                rep.tick(false);
                return Ok(());
            }
            rep.tick(false);
            copy_file(src, dst, rep)
        }
    }
}

fn remove_rec(l: &Loc, rep: &mut Reporter) -> io::Result<()> {
    rep.check()?;
    match l.kind()? {
        Some(Kind::Dir) => {
            for n in l.names()? {
                remove_rec(&l.child(&n), rep)?;
            }
            l.remove_dir()?;
        }
        // remove the link itself, never what it points to
        Some(Kind::Link) => l.remove_file().or_else(|_| l.remove_dir())?,
        Some(Kind::File(_)) => l.remove_file()?,
        None => {}
    }
    rep.done += 1;
    rep.current = name_of(&l.label());
    rep.tick(false);
    Ok(())
}

/// Same rule as for local folders: a single source may get a new name, several go
/// into `dest` as a folder (created if needed).
fn pairs(sources: &[PathBuf], dest: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let d = loc(dest).map_err(|e| err_msg(e, dest))?;
    if sources.len() > 1 && !d.is_dir() {
        d.mkdir_all().map_err(|e| err_msg(e, dest))?;
    }
    let into = d.is_dir();
    Ok(sources.iter().map(|s| (s.clone(), if into { join(dest, &name_of(s)) } else { dest.to_path_buf() })).collect())
}

fn inside(child: &Path, parent: &Path) -> bool {
    if remote::is_remote(child) || remote::is_remote(parent) {
        remote::starts_with(child, parent)
    } else {
        child.starts_with(parent)
    }
}

fn same(a: &Path, b: &Path) -> bool {
    match (remote::normalize(a), remote::normalize(b)) {
        (Some(x), Some(y)) => remote::split(&x) == remote::split(&y),
        (None, None) => a == b,
        _ => false,
    }
}

fn summary(verb: &str, n: usize, skipped: usize) -> String {
    let mut s = format!("{verb}: {}", quokka_domain::format::items_text(n as u64));
    if skipped > 0 {
        s.push_str(&trf!(", пропущено существующих: {skipped}", ", existing skipped: {skipped}"));
    }
    s
}

pub fn copy(sources: &[PathBuf], dest: &Path, overwrite: bool, rep: &mut Reporter) -> Result<String, String> {
    let pairs = pairs(sources, dest)?;
    let mut total = 0;
    for (s, _) in &pairs {
        let l = loc(s).map_err(|e| err_msg(e, s))?;
        total += tree_size(&l, rep).map_err(|e| err_msg(e, s))?.0;
    }
    rep.total = total;
    rep.tick(true);
    for (s, t) in &pairs {
        if same(s, t) {
            return Err(trf!("«{}» уже находится в этой папке", "“{}” is already in this folder", name_of(s)));
        }
        if inside(t, s) {
            return Err(trf!("Нельзя скопировать «{}» в саму себя", "Cannot copy “{}” into itself", name_of(s)));
        }
        let (src, dst) = (loc(s).map_err(|e| err_msg(e, s))?, loc(t).map_err(|e| err_msg(e, t))?);
        copy_rec(&src, &dst, overwrite, rep).map_err(|e| err_msg(e, s))?;
    }
    Ok(summary(tr!("Скопировано", "Copied"), pairs.len(), rep.skipped))
}

/// Within one server a move is a rename; otherwise copy, then delete the source.
pub fn move_items(sources: &[PathBuf], dest: &Path, overwrite: bool, rep: &mut Reporter) -> Result<String, String> {
    let pairs = pairs(sources, dest)?;
    rep.total = pairs.len() as u64;
    rep.tick(true);
    let mut moved = 0;
    for (i, (s, t)) in pairs.iter().enumerate() {
        rep.check().map_err(|e| err_msg(e, s))?;
        rep.current = name_of(s);
        rep.done = i as u64;
        rep.tick(true);
        if same(s, t) {
            continue;
        }
        if inside(t, s) {
            return Err(trf!("Нельзя переместить «{}» в саму себя", "Cannot move “{}” into itself", name_of(s)));
        }
        let (src, dst) = (loc(s).map_err(|e| err_msg(e, s))?, loc(t).map_err(|e| err_msg(e, t))?);
        if !src.exists() {
            rep.skipped += 1;
            continue;
        }
        let target_exists = dst.exists();
        if target_exists && !overwrite {
            rep.skipped += 1;
            continue;
        }
        if let (Loc::Remote(a), Loc::Remote(b)) = (&src, &dst) {
            if a.same_server(b) && !target_exists && a.rename(b).is_ok() {
                rep.moved.push((s.clone(), t.clone()));
                moved += 1;
                continue;
            }
        }
        let skipped_before = rep.skipped;
        let copied = copy_rec(&src, &dst, overwrite, rep);
        if copied.is_err() && !target_exists {
            let _ = remove_rec(&dst, rep);
        }
        copied.map_err(|e| err_msg(e, s))?;
        if rep.skipped != skipped_before {
            // something inside was not copied: keep the source rather than lose it
            continue;
        }
        if !target_exists {
            rep.moved.push((s.clone(), t.clone()));
        }
        remove_rec(&src, rep).map_err(|e| err_msg(e, s))?;
        rep.done = i as u64 + 1;
        moved += 1;
    }
    rep.done = rep.total;
    rep.tick(true);
    Ok(summary(tr!("Перемещено", "Moved"), moved, rep.skipped))
}

pub fn delete(sources: &[PathBuf], rep: &mut Reporter) -> Result<String, String> {
    let mut total = 0;
    for s in sources {
        let l = loc(s).map_err(|e| err_msg(e, s))?;
        total += tree_size(&l, rep).map_err(|e| err_msg(e, s))?.1;
    }
    rep.total = total;
    rep.tick(true);
    for s in sources {
        let l = loc(s).map_err(|e| err_msg(e, s))?;
        remove_rec(&l, rep).map_err(|e| err_msg(e, s))?;
    }
    Ok(summary(tr!("Удалено", "Deleted"), sources.len(), 0))
}

// ───────────────────────────── single-item helpers for the UI ─────────────────────────────

pub fn exists(p: &Path) -> bool {
    loc(p).map(|l| l.exists()).unwrap_or(false)
}

pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    match (loc(from)?, loc(to)?) {
        (Loc::Remote(a), Loc::Remote(b)) if a.same_server(&b) => a.rename(&b),
        (Loc::Local(a), Loc::Local(b)) => fs::rename(a, b),
        _ => Err(io::Error::new(io::ErrorKind::Unsupported, tr!("Переименование между разными местами не поддерживается", "Cannot rename across locations"))),
    }
}

pub fn mkdir_all(p: &Path) -> io::Result<()> {
    loc(p)?.mkdir_all()
}

pub fn create_file(p: &Path) -> io::Result<()> {
    match loc(p)? {
        Loc::Local(p) => fs::OpenOptions::new().write(true).create_new(true).open(p).map(|_| ()),
        Loc::Remote(r) => r.create_new(),
    }
}

/// Read a whole remote file (up to `limit` bytes) — for the viewer.
pub fn read_limited(p: &Path, limit: usize) -> io::Result<(Vec<u8>, u64)> {
    let l = loc(p)?;
    let total = match l.kind()? {
        Some(Kind::File(n)) => n,
        _ => return Err(io::ErrorKind::NotFound.into()),
    };
    let mut data = Vec::new();
    l.reader()?.take(limit as u64).read_to_end(&mut data)?;
    Ok((data, total))
}

/// Replace a remote file's content (built-in editor save).
pub fn write_all(p: &Path, data: &[u8]) -> io::Result<()> {
    match loc(p)? {
        Loc::Local(p) => fs::write(p, data),
        Loc::Remote(r) => {
            let mut w = r.writer()?;
            w.write_all(data)?;
            w.finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sftp::tests::served;
    use quokka_core::job::Job;
    use quokka_core::Waker;

    fn run(f: impl FnOnce(&mut Reporter) -> Result<String, String> + Send + 'static) -> Result<String, String> {
        let mut job = Job::spawn("t".into(), true, Waker::noop(), f);
        for _ in 0..1000 {
            job.poll();
            if let Some(r) = job.finished.take() {
                return r;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("job did not finish");
    }

    #[test]
    fn sftp_upload_download_move_delete() {
        let (root, site) = served("xfer");
        round_trip(root, site);
    }

    #[test]
    fn ftp_upload_download_move_delete() {
        let (root, site) = crate::ftp_test_server::served("xfer");
        round_trip(root, site);
    }

    /// The server's `/home` is `root/home` for both test servers.
    fn round_trip(root: PathBuf, site: quokka_domain::remote::Site) {
        let local = root.with_extension("local");
        let _ = fs::remove_dir_all(&local);
        fs::create_dir_all(local.join("src/вложенная")).unwrap();
        let big: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(local.join("src/big.bin"), &big).unwrap();
        fs::write(local.join("src/вложенная/b.txt"), b"bee").unwrap();
        let home = site.path("/home");

        // upload a folder
        let (s, d) = (vec![local.join("src")], home.clone());
        run(move |rep| copy(&s, &d, false, rep)).unwrap();
        assert_eq!(fs::read(root.join("home/src/big.bin")).unwrap(), big);
        assert_eq!(fs::read(root.join("home/src/вложенная/b.txt")).unwrap(), b"bee");

        // existing files are skipped without overwrite
        let (s, d) = (vec![local.join("src")], home.clone());
        let msg = run(move |rep| copy(&s, &d, false, rep)).unwrap();
        assert!(msg.contains('2'), "{msg}");

        // download it back under another name
        let (s, d) = (vec![home.join("src")], local.join("back"));
        run(move |rep| copy(&s, &d, false, rep)).unwrap();
        assert_eq!(fs::read(local.join("back/big.bin")).unwrap(), big);

        // move within the server = rename
        let (s, d) = (vec![site.path("/home/src/вложенная")], site.path("/moved"));
        run(move |rep| move_items(&s, &d, false, rep)).unwrap();
        assert!(root.join("moved/b.txt").is_file() && !root.join("home/src/вложенная").exists());

        // move from the server to the disk
        let (s, d) = (vec![site.path("/moved")], local.clone());
        run(move |rep| move_items(&s, &d, false, rep)).unwrap();
        assert!(local.join("moved/b.txt").is_file() && !root.join("moved").exists());

        // copying into itself is refused
        let (s, d) = (vec![home.join("src")], home.join("src").join("x"));
        assert!(run(move |rep| copy(&s, &d, false, rep)).is_err());

        // delete on the server
        let s = vec![home.join("src")];
        run(move |rep| delete(&s, rep)).unwrap();
        assert!(!root.join("home/src").exists());

        // single-item helpers
        let f = home.join("new.txt");
        create_file(&f).unwrap();
        assert!(create_file(&f).is_err());
        write_all(&f, b"text").unwrap();
        assert_eq!(read_limited(&f, 2).unwrap(), (b"te".to_vec(), 4));
        rename(&f, &home.join("renamed.txt")).unwrap();
        assert!(exists(&home.join("renamed.txt")) && !exists(&f));
        mkdir_all(&home.join("a/b/c")).unwrap();
        assert!(root.join("home/a/b/c").is_dir());

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&local);
    }
}
