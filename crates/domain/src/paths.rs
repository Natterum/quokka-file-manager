//! Path rules shared by navigation and the UI.

use std::path::{Path, PathBuf};

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
