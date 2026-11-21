//! What a background file operation should do, independent of how it is carried out.

use crate::trf;
use crate::format::items_text;
use crate::imgconv::ConvertOptions;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub enum OpSpec {
    Copy { sources: Vec<PathBuf>, dest: PathBuf, overwrite: bool },
    Move { sources: Vec<PathBuf>, dest: PathBuf, overwrite: bool },
    Delete { sources: Vec<PathBuf> },
    Recycle { sources: Vec<PathBuf> },
    /// Restore items sent to the Recycle Bin (by their original paths).
    Restore { originals: Vec<PathBuf> },
    /// Undo of a move: put each `(current, original)` item back at its original path.
    MoveBack { pairs: Vec<(PathBuf, PathBuf)> },
    Zip { sources: Vec<PathBuf>, dest: PathBuf },
    Unzip { archive: PathBuf, dest: PathBuf, overwrite: bool },
    /// Convert images (folders are searched recursively) into `dest`, keeping the
    /// folder structure below each selected folder.
    ConvertImages { sources: Vec<PathBuf>, dest: PathBuf, options: ConvertOptions },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpKind {
    Copy,
    Move,
    Delete,
    Archive,
    Convert,
}

fn what(sources: &[PathBuf]) -> String {
    match sources {
        [one] => format!("«{}»", file_name(one)),
        _ => items_text(sources.len() as u64),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn folder(p: &Path) -> String {
    let s = file_name(p);
    if s.is_empty() {
        p.display().to_string()
    } else {
        s
    }
}

impl OpSpec {
    pub fn title(&self) -> String {
        match self {
            OpSpec::Copy { sources, dest, .. } => trf!("Копирование {} в «{}»", "Copying {} to “{}”", what(sources), folder(dest)),
            OpSpec::Move { sources, dest, .. } => trf!("Перемещение {} в «{}»", "Moving {} to “{}”", what(sources), folder(dest)),
            OpSpec::Delete { sources } => trf!("Удаление {}", "Deleting {}", what(sources)),
            OpSpec::Recycle { sources } => trf!("В корзину: {}", "To Recycle Bin: {}", what(sources)),
            OpSpec::Restore { originals } => trf!("Восстановление {} из корзины", "Restoring {} from Recycle Bin", what(originals)),
            OpSpec::MoveBack { pairs } => {
                let originals: Vec<PathBuf> = pairs.iter().map(|(_, o)| o.clone()).collect();
                trf!("Отмена перемещения {}", "Undoing move of {}", what(&originals))
            }
            OpSpec::Zip { sources, dest } => trf!("Архивация {} в «{}»", "Zipping {} to “{}”", what(sources), file_name(dest)),
            OpSpec::Unzip { archive, dest, .. } => trf!("Распаковка «{}» в «{}»", "Extracting “{}” to “{}”", file_name(archive), folder(dest)),
            OpSpec::ConvertImages { sources, dest, options } => {
                trf!("Конвертация {} в {} → «{}»", "Converting {} to {} → “{}”", what(sources), options.describe(), folder(dest))
            }
        }
    }

    pub fn kind(&self) -> OpKind {
        match self {
            OpSpec::Copy { .. } => OpKind::Copy,
            OpSpec::Move { .. } => OpKind::Move,
            OpSpec::Delete { .. } | OpSpec::Recycle { .. } => OpKind::Delete,
            OpSpec::Restore { .. } | OpSpec::MoveBack { .. } => OpKind::Move,
            OpSpec::Zip { .. } | OpSpec::Unzip { .. } => OpKind::Archive,
            OpSpec::ConvertImages { .. } => OpKind::Convert,
        }
    }

    /// Folders whose listing this operation can change: where the sources were and
    /// where the results go. Used to refresh only the affected tabs.
    pub fn touched_dirs(&self) -> Vec<PathBuf> {
        let parents = |v: &[PathBuf]| v.iter().filter_map(|p| p.parent().map(Path::to_path_buf)).collect::<Vec<_>>();
        let mut out = match self {
            OpSpec::Copy { dest, .. } => vec![dest.clone()],
            OpSpec::Move { sources, dest, .. } => {
                let mut v = parents(sources);
                v.push(dest.clone());
                v
            }
            OpSpec::Delete { sources } | OpSpec::Recycle { sources } => parents(sources),
            OpSpec::Restore { originals } => parents(originals),
            OpSpec::MoveBack { pairs } => {
                let all: Vec<PathBuf> = pairs.iter().flat_map(|(a, b)| [a.clone(), b.clone()]).collect();
                parents(&all)
            }
            OpSpec::Zip { dest, .. } => parents(std::slice::from_ref(dest)),
            OpSpec::Unzip { dest, .. } | OpSpec::ConvertImages { dest, .. } => vec![dest.clone()],
        };
        // a destination that did not exist yet is created inside its parent
        let extra: Vec<PathBuf> = out.iter().filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
        out.extend(extra);
        out.sort();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touched_dirs_cover_sources_and_destination() {
        let spec = OpSpec::Move {
            sources: vec![PathBuf::from("/a/x.txt"), PathBuf::from("/a/y.txt")],
            dest: PathBuf::from("/b/new"),
            overwrite: false,
        };
        let dirs = spec.touched_dirs();
        assert!(dirs.contains(&PathBuf::from("/a")));
        assert!(dirs.contains(&PathBuf::from("/b/new")));
        assert!(dirs.contains(&PathBuf::from("/b")));
    }
}
