//! Renames a batch of files on disk (group rename, its undo). Safe for chains and
//! swaps (`a → b`, `b → a`): when a target is another item of the batch, everything
//! goes through temporary names first. On failure what was already renamed is put back.

use quokka_domain::trf;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

fn temp_name(p: &Path, i: usize) -> PathBuf {
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    p.with_file_name(format!(".{name}.quokka-{}-{i}.tmp", std::process::id()))
}

/// Undo `done` (pairs already renamed `from → to`), newest first; best effort.
fn roll_back(done: &[(PathBuf, PathBuf)]) {
    for (from, to) in done.iter().rev() {
        let _ = fs::rename(to, from);
    }
}

/// Rename every `(from, to)`. Returns the pairs that were applied (all of them), or
/// an error after restoring the original names.
pub fn rename_batch(pairs: &[(PathBuf, PathBuf)]) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let pairs: Vec<(PathBuf, PathBuf)> = pairs.iter().filter(|(a, b)| a != b).cloned().collect();
    let sources: HashSet<String> = pairs.iter().map(|(a, _)| key(a)).collect();
    // a target that is another source (not a case-only change of itself) needs the 2-step way
    let chained = pairs.iter().any(|(a, b)| key(a) != key(b) && sources.contains(&key(b)));
    let fail = |done: &[(PathBuf, PathBuf)], from: &Path, e: std::io::Error| {
        roll_back(done);
        let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        trf!("Не удалось переименовать «{name}»: {e}. Изменения отменены", "Failed to rename “{name}”: {e}. Changes reverted")
    };

    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    if !chained {
        for (from, to) in &pairs {
            if key(from) != key(to) && to.symlink_metadata().is_ok() {
                roll_back(&done);
                return Err(trf!("«{}» уже существует. Изменения отменены", "“{}” already exists. Changes reverted", to.display()));
            }
            fs::rename(from, to).map_err(|e| fail(&done, from, e))?;
            done.push((from.clone(), to.clone()));
        }
        return Ok(done);
    }

    // step 1: everything to temporary names, step 2: temporary names to the targets
    let mut temps = Vec::with_capacity(pairs.len());
    for (i, (from, _)) in pairs.iter().enumerate() {
        let tmp = temp_name(from, i);
        fs::rename(from, &tmp).map_err(|e| fail(&done, from, e))?;
        done.push((from.clone(), tmp.clone()));
        temps.push(tmp);
    }
    for ((from, to), tmp) in pairs.iter().zip(&temps) {
        if to.symlink_metadata().is_ok() {
            roll_back(&done);
            return Err(trf!("«{}» уже существует. Изменения отменены", "“{}” already exists. Changes reverted", to.display()));
        }
        fs::rename(tmp, to).map_err(|e| fail(&done, from, e))?;
        done.push((tmp.clone(), to.clone()));
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("quokka-rename-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn simple_swap_and_undo() {
        let d = dir("swap");
        fs::write(d.join("a.txt"), "A").unwrap();
        fs::write(d.join("b.txt"), "B").unwrap();
        let pairs = vec![(d.join("a.txt"), d.join("b.txt")), (d.join("b.txt"), d.join("a.txt"))];
        let done = rename_batch(&pairs).unwrap();
        assert_eq!(fs::read_to_string(d.join("a.txt")).unwrap(), "B");
        assert_eq!(fs::read_to_string(d.join("b.txt")).unwrap(), "A");
        let undo: Vec<_> = done.into_iter().map(|(a, b)| (b, a)).collect();
        rename_batch(&undo).unwrap();
        assert_eq!(fs::read_to_string(d.join("a.txt")).unwrap(), "A");
        assert_eq!(names(&d), ["a.txt", "b.txt"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn case_only_and_plain() {
        let d = dir("case");
        fs::write(d.join("x.txt"), "").unwrap();
        fs::write(d.join("y.txt"), "").unwrap();
        rename_batch(&[(d.join("x.txt"), d.join("X.TXT")), (d.join("y.txt"), d.join("z.txt"))]).unwrap();
        assert_eq!(names(&d), ["X.TXT", "z.txt"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn failure_rolls_back() {
        let d = dir("fail");
        fs::write(d.join("1.txt"), "").unwrap();
        fs::write(d.join("2.txt"), "").unwrap();
        fs::write(d.join("taken.txt"), "").unwrap();
        let err = rename_batch(&[(d.join("1.txt"), d.join("one.txt")), (d.join("2.txt"), d.join("taken.txt"))]).unwrap_err();
        assert!(err.contains("уже существует"), "{err}");
        assert_eq!(names(&d), ["1.txt", "2.txt", "taken.txt"]);
        fs::remove_dir_all(&d).unwrap();
    }
}
