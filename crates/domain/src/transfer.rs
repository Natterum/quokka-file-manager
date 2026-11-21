//! Rules for dropping files: copy or move, and where a drop is allowed.
//!
//! Copy vs move follows the Windows convention: Ctrl — copy, Shift — move, no key —
//! move within one drive, copy between drives.

use std::path::{Component, Path, PathBuf};

/// Copy or move for a drop of `sources` into `dest` with the given keys held.
pub fn is_move(ctrl: bool, shift: bool, sources: &[PathBuf], dest: &Path) -> bool {
    if ctrl {
        false
    } else if shift {
        true
    } else {
        sources.first().is_some_and(|s| same_volume(s, dest))
    }
}

fn volume(p: &Path) -> Option<String> {
    match p.components().next()? {
        Component::Prefix(pre) => Some(pre.as_os_str().to_string_lossy().to_lowercase()),
        Component::RootDir => Some("/".into()),
        _ => None,
    }
}

pub fn same_volume(a: &Path, b: &Path) -> bool {
    match (volume(a), volume(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Whether `sources` may be dropped into `dest`: not into a source itself or one of
/// its subfolders, and not back into the folder they already are in.
pub fn can_drop(sources: &[PathBuf], dest: &Path) -> bool {
    if dest.as_os_str().is_empty() || sources.is_empty() {
        return false;
    }
    let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    let d = lower(dest);
    let all_here = sources.iter().all(|s| s.parent().is_some_and(|p| lower(p) == d));
    !all_here && !sources.iter().any(|s| d.starts_with(lower(s)))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn v(p: &str) -> Vec<PathBuf> {
        vec![PathBuf::from(p)]
    }

    #[test]
    fn keys_pick_copy_or_move() {
        let src = v(r"C:\a\file.txt");
        assert!(is_move(false, false, &src, Path::new(r"c:\b")), "same drive — move");
        assert!(!is_move(false, false, &src, Path::new(r"D:\b")), "other drive — copy");
        assert!(!is_move(true, false, &src, Path::new(r"C:\b")), "Ctrl — copy");
        assert!(is_move(false, true, &src, Path::new(r"D:\b")), "Shift — move");
    }

    #[test]
    fn drop_rules() {
        assert!(can_drop(&v(r"C:\a\x"), Path::new(r"C:\b")));
        assert!(!can_drop(&v(r"C:\a\x"), Path::new(r"C:\A")), "same folder");
        assert!(!can_drop(&v(r"C:\a\x"), Path::new(r"C:\a\x\sub")), "into itself");
        assert!(!can_drop(&v(r"C:\a\x"), Path::new("")), "This PC");
    }
}
