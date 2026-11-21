//! Read-only ZIP folders. Virtual paths are `archive.zip/member/path`;
//! only the central directory is read for navigation, never the file payloads.
use crate::zip::ZipReader;
use quokka_core::job::Reporter;
use quokka_domain::{entry::Meta, tr, Entry};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub fn location(path: &Path) -> Option<(PathBuf, PathBuf)> {
    path.ancestors()
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) && p.is_file())
        .map(|archive| {
            (
                archive.to_path_buf(),
                path.strip_prefix(archive).unwrap().to_path_buf(),
            )
        })
}

pub fn is_member(path: &Path) -> bool {
    location(path).is_some_and(|(_, member)| !member.as_os_str().is_empty())
}

fn missing() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        tr!("Объект в архиве не найден", "Archive member not found"),
    )
}

pub fn read_dir(path: &Path, cancel: &AtomicBool) -> io::Result<Vec<Entry>> {
    let (archive, prefix) = location(path).ok_or_else(missing)?;
    let reader = ZipReader::open(&archive)?;
    let mut children: BTreeMap<PathBuf, Entry> = BTreeMap::new();
    let mut found = prefix.as_os_str().is_empty();
    for e in &reader.entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let Some(relative) = e.safe_path() else {
            continue;
        };
        if relative == prefix && e.dir {
            found = true;
        }
        let Ok(rest) = relative.strip_prefix(&prefix) else {
            continue;
        };
        let mut parts = rest.components();
        let Some(first) = parts.next() else { continue };
        found = true;
        let name = first.as_os_str().to_string_lossy().into_owned();
        let is_dir = parts.next().is_some() || e.dir;
        let virtual_path = path.join(first.as_os_str());
        let item = Entry::item(
            name.clone(),
            &name,
            virtual_path.clone(),
            Meta {
                is_dir,
                size: e.size,
                modified: e.modified(),
                hidden: name.starts_with('.'),
                ..Meta::default()
            },
        );
        children
            .entry(virtual_path)
            .and_modify(|old| {
                if is_dir {
                    *old = item.clone();
                }
            })
            .or_insert(item);
    }
    if !found {
        return Err(missing());
    }
    Ok(children.into_values().collect())
}

pub fn stat(path: &Path) -> Option<Meta> {
    let (archive, member) = location(path)?;
    if member.as_os_str().is_empty() {
        return Some(Meta {
            is_dir: true,
            size: 0,
            modified: None,
            hidden: false,
            ..Meta::default()
        });
    }
    let reader = ZipReader::open(&archive).ok()?;
    for e in &reader.entries {
        let Some(p) = e.safe_path() else { continue };
        if p == member || p.starts_with(&member) {
            let is_dir = e.dir || p != member;
            return Some(Meta {
                is_dir,
                size: if is_dir { 0 } else { e.size },
                modified: e.modified(),
                hidden: false,
                ..Meta::default()
            });
        }
    }
    None
}

/// Bounded preview input, including virtual members. Prefix previews may stop
/// before CRC validation; full reads always validate CRC and reject oversized data.
pub fn read(
    path: &Path,
    limit: usize,
    prefix: bool,
    cancel: &AtomicBool,
) -> io::Result<(Vec<u8>, u64)> {
    if let Some((archive, member)) = location(path).filter(|(_, m)| !m.as_os_str().is_empty()) {
        let mut reader = ZipReader::open(&archive)?;
        let index = reader
            .entries
            .iter()
            .position(|e| !e.dir && e.safe_path().as_deref() == Some(member.as_path()))
            .ok_or_else(missing)?;
        let size = reader.entries[index].size;
        if !prefix && size > limit as u64 {
            return Err(io::Error::other(tr!(
                "Файл слишком большой для просмотра",
                "File is too large to preview"
            )));
        }
        let mut out = Bounded {
            data: Vec::new(),
            limit,
        };
        let result = reader.extract(index, &mut out, |_| {
            if cancel.load(Ordering::Relaxed) {
                Err(io::ErrorKind::Interrupted.into())
            } else {
                Ok(())
            }
        });
        match result {
            Err(e)
                if prefix && e.kind() == io::ErrorKind::FileTooLarge && out.data.len() == limit => {
            }
            other => other?,
        }
        Ok((out.data, size))
    } else {
        let file = fs::File::open(path)?;
        let size = file.metadata()?.len();
        if !prefix && size > limit as u64 {
            return Err(io::Error::other(tr!(
                "Файл слишком большой для просмотра",
                "File is too large to preview"
            )));
        }
        let mut data = Vec::new();
        file.take(limit as u64).read_to_end(&mut data)?;
        Ok((data, size))
    }
}

struct Bounded {
    data: Vec<u8>,
    limit: usize,
}
impl Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let count = buf.len().min(self.limit - self.data.len());
        if count == 0 {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        self.data.extend_from_slice(&buf[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn size(path: &Path) -> io::Result<u64> {
    let (archive, member) = location(path).ok_or_else(missing)?;
    let reader = ZipReader::open(&archive)?;
    Ok(reader
        .entries
        .iter()
        .filter(|e| {
            !e.dir
                && e.safe_path()
                    .is_some_and(|p| p == member || p.starts_with(&member))
        })
        .map(|e| e.size)
        .sum())
}

/// Do not follow existing links/junctions when writing archive contents.
fn check_target(path: &Path) -> io::Result<()> {
    for parent in path.ancestors() {
        if let Ok(meta) = fs::symlink_metadata(parent) {
            #[cfg(windows)]
            let link = {
                use std::os::windows::fs::MetadataExt;
                meta.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let link = meta.file_type().is_symlink();
            if link {
                return Err(io::Error::other(tr!(
                    "Распаковка через ссылки запрещена",
                    "Cannot extract through links"
                )));
            }
        }
    }
    Ok(())
}

pub fn extract_to(
    source: &Path,
    dest: &Path,
    overwrite: bool,
    rep: &mut Reporter,
) -> io::Result<()> {
    let (archive, member) = location(source).ok_or_else(missing)?;
    let mut reader = ZipReader::open(&archive)?;
    let mut found = false;
    for i in 0..reader.entries.len() {
        rep.check()?;
        let e = &reader.entries[i];
        let Some(relative) = e.safe_path() else {
            continue;
        };
        let target = if relative == member {
            dest.to_path_buf()
        } else if let Ok(rest) = relative.strip_prefix(&member) {
            dest.join(rest)
        } else {
            continue;
        };
        found = true;
        check_target(&target)?;
        if e.dir {
            fs::create_dir_all(&target)?;
            continue;
        }
        rep.current = e.name.clone();
        if target.exists() && !overwrite {
            rep.skipped += 1;
            rep.done += e.size;
            rep.tick(false);
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        // A staging file keeps the destination intact if CRC validation or
        // cancellation fails. create_new also prevents following a planted link.
        let parent = target.parent().ok_or_else(missing)?;
        let mut attempt = 0u64;
        let (staging, mut output) = loop {
            rep.check()?;
            let name = parent.join(format!(
                ".quokka-extract-{}-{attempt}.tmp",
                std::process::id()
            ));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&name)
            {
                Ok(file) => break (name, file),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => attempt += 1,
                Err(e) => return Err(e),
            }
        };
        let result = reader.extract(i, &mut output, |n| {
            rep.check()?;
            rep.done += n;
            rep.tick(false);
            Ok(())
        });
        let result = result.and_then(|_| output.flush());
        drop(output);
        let result = result.and_then(|_| {
            rep.check()?;
            check_target(&target)?;
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                let from: Vec<u16> = staging.as_os_str().encode_wide().chain(Some(0)).collect();
                let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
                if unsafe {
                    windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                        from.as_ptr(),
                        to.as_ptr(),
                        if overwrite { 1 } else { 0 },
                    )
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            }
            #[cfg(not(windows))]
            {
                if overwrite {
                    fs::rename(&staging, &target)
                } else {
                    fs::hard_link(&staging, &target)
                }
            }
        });
        let _ = fs::remove_file(&staging);
        result?;
    }
    if !found {
        return Err(missing());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fsops::StdFileOps, zip::ZipWriter};
    use quokka_core::{panel::Panel, ports::FileOps, Waker};
    use quokka_domain::OpSpec;

    struct Fixture {
        root: PathBuf,
        zip: PathBuf,
    }
    impl Fixture {
        fn new(label: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("quokka-archive-{label}-{}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            let zip = root.join("test.zip");
            let mut writer = ZipWriter::create(&zip).unwrap();
            // Implicit folders, an explicit empty folder, Unicode, unsafe paths.
            for (name, bytes) in [
                ("docs/nested/a.txt", "hello"),
                ("docs/b.txt", "world"),
                ("фото.txt", "данные"),
                ("../evil.txt", "bad"),
                ("/absolute.txt", "bad"),
                ("CON.txt", "bad"),
            ] {
                writer
                    .add_file(name, &mut bytes.as_bytes(), None, |_| Ok(()))
                    .unwrap();
            }
            writer.add_dir("empty/", None).unwrap();
            writer
                .add_file("big.txt", &mut &vec![b'x'; 1024 * 1024][..], None, |_| {
                    Ok(())
                })
                .unwrap();
            writer.finish().unwrap();
            Self { root, zip }
        }
        fn copy(
            &self,
            sources: Vec<PathBuf>,
            dest: PathBuf,
            overwrite: bool,
        ) -> Result<String, String> {
            let mut job = StdFileOps.start(
                &OpSpec::Copy {
                    sources,
                    dest,
                    overwrite,
                },
                Waker::noop(),
            );
            for _ in 0..500 {
                job.poll();
                if let Some(result) = job.finished.take() {
                    return result;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("extraction timed out");
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn folders_navigation_history_filtering_and_bounded_reads() {
        let f = Fixture::new("browse");
        let cancel = AtomicBool::new(false);
        let names: Vec<String> = read_dir(&f.zip, &cancel)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["big.txt", "docs", "empty", "фото.txt"]);
        assert!(read_dir(&f.zip.join("empty"), &cancel).unwrap().is_empty());
        assert!(read_dir(&f.zip.join("missing"), &cancel).is_err());
        assert!(stat(&f.zip.join("docs")).unwrap().is_dir);
        let mut p = Panel::new(f.root.clone(), false, crate::services(Waker::noop()));
        p.navigate(f.zip.clone()).unwrap();
        p.wait_loaded();
        p.navigate(f.zip.join("docs")).unwrap();
        p.wait_loaded();
        assert_eq!(p.files, 1);
        assert_eq!(p.dirs, 1);
        p.go_up().unwrap();
        p.wait_loaded();
        assert_eq!(p.current().unwrap().name, "docs");
        p.go_back().unwrap();
        p.wait_loaded();
        assert_eq!(p.path, f.zip.join("docs"));
        p.go_forward().unwrap();
        p.wait_loaded();
        p.filter = ".txt".into();
        p.apply_filter();
        assert_eq!(p.visible.len(), 2);
        assert_eq!(
            read(&f.zip.join("docs/nested/a.txt"), 20, false, &cancel)
                .unwrap()
                .0,
            b"hello"
        );
        let (prefix, size) = read(&f.zip.join("big.txt"), 512, true, &cancel).unwrap();
        assert_eq!(prefix, vec![b'x'; 512]);
        assert_eq!(size, 1024 * 1024);
        assert!(read(&f.zip.join("big.txt"), 512, false, &cancel).is_err());
        cancel.store(true, Ordering::Relaxed);
        assert!(read_dir(&f.zip, &cancel).is_err());
    }

    #[test]
    fn selected_extraction_handles_nested_empty_unicode_conflicts_and_readonly() {
        let f = Fixture::new("extract");
        let out = f.root.join("out");
        f.copy(
            vec![
                f.zip.join("docs"),
                f.zip.join("empty"),
                f.zip.join("фото.txt"),
            ],
            out.clone(),
            false,
        )
        .unwrap();
        assert_eq!(fs::read(out.join("docs/nested/a.txt")).unwrap(), b"hello");
        assert!(out.join("empty").is_dir());
        assert_eq!(fs::read_to_string(out.join("фото.txt")).unwrap(), "данные");
        assert!(!out.join("big.txt").exists());
        fs::write(out.join("фото.txt"), b"keep").unwrap();
        f.copy(vec![f.zip.join("фото.txt")], out.clone(), false)
            .unwrap();
        assert_eq!(fs::read(out.join("фото.txt")).unwrap(), b"keep");
        f.copy(vec![f.zip.join("фото.txt")], out.clone(), true)
            .unwrap();
        assert_eq!(fs::read_to_string(out.join("фото.txt")).unwrap(), "данные");
        let renamed = f.root.join("renamed.txt");
        f.copy(vec![f.zip.join("docs/b.txt")], renamed.clone(), false)
            .unwrap();
        assert_eq!(fs::read(renamed).unwrap(), b"world");
        assert!(f
            .copy(vec![f.zip.join("docs")], f.zip.join("new"), false)
            .is_err());
        assert!(crate::fsops::validate_operation(&OpSpec::Move {
            sources: vec![f.zip.join("docs")],
            dest: out,
            overwrite: false
        })
        .is_err());
        assert!(crate::fsops::validate_operation(&OpSpec::Delete {
            sources: vec![f.zip.join("docs")]
        })
        .is_err());
        assert!(!f.root.join("evil.txt").exists());
    }

    #[test]
    fn corrupt_crc_preserves_existing_destination_and_cleans_staging() {
        let f = Fixture::new("crc");
        let mut bytes = fs::read(&f.zip).unwrap();
        let central = bytes
            .windows(4)
            .position(|b| b == [0x50, 0x4b, 0x01, 0x02])
            .unwrap();
        bytes[central + 16] ^= 1;
        fs::write(&f.zip, bytes).unwrap();
        let dest = f.root.join("keep.txt");
        fs::write(&dest, b"keep").unwrap();
        assert!(f
            .copy(vec![f.zip.join("docs/nested/a.txt")], dest.clone(), true)
            .is_err());
        assert_eq!(fs::read(dest).unwrap(), b"keep");
        assert!(fs::read_dir(&f.root).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".quokka-extract")));
    }
}
