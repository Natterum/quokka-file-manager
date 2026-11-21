//! One row of a file list, plus sorting and filtering rules for lists of rows.

use crate::{tr, trf};
use crate::kind::{ext_of, FileKind};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub lower: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_parent: bool,
    pub is_drive: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    /// File attribute letters (R/H/S/A/D/L/C/E); None when unavailable.
    pub attributes: Option<String>,
    pub kind: FileKind,
    pub ext: String,
    pub hidden: bool,
    /// Shared between all entries of the same type, so big folders hold one copy.
    pub type_name: Arc<str>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Modified,
    Size,
    Type,
    Created,
    Accessed,
    Extension,
    Attributes,
}

/// What a file system reports about one item; turned into an [`Entry`] by [`Entry::item`].
#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    /// File attribute letters (R/H/S/A/D/L/C/E); None when unavailable.
    pub attributes: Option<String>,
    pub hidden: bool,
}

impl Entry {
    /// A file or folder. `name` is what the list shows (for search results — a path
    /// relative to the search root), `file_name` the real name the type comes from.
    pub fn item(name: String, file_name: &str, path: PathBuf, meta: Meta) -> Entry {
        let ext = if meta.is_dir { String::new() } else { ext_of(file_name) };
        let kind = if meta.is_dir { FileKind::Folder } else { FileKind::from_ext(&ext) };
        Entry {
            lower: name.to_lowercase(),
            name,
            path,
            is_dir: meta.is_dir,
            is_parent: false,
            is_drive: false,
            size: if meta.is_dir { 0 } else { meta.size },
            modified: meta.modified,
            created: meta.created,
            accessed: meta.accessed,
            attributes: meta.attributes,
            kind,
            type_name: type_name(&ext, kind, meta.is_dir),
            ext,
            hidden: meta.hidden,
        }
    }

    /// The ".." row leading to `path`.
    pub fn parent(path: PathBuf) -> Entry {
        Entry {
            name: "..".into(),
            lower: "..".into(),
            path,
            is_dir: true,
            is_parent: true,
            is_drive: false,
            size: 0,
            modified: None,
            created: None,
            accessed: None,
            attributes: None,
            kind: FileKind::Folder,
            ext: String::new(),
            hidden: false,
            type_name: intern(""),
        }
    }

    /// A drive in the "This PC" view.
    pub fn drive(name: String, path: PathBuf, total: u64) -> Entry {
        Entry {
            lower: name.to_lowercase(),
            name,
            path,
            is_dir: true,
            is_parent: false,
            is_drive: true,
            size: total,
            modified: None,
            created: None,
            accessed: None,
            attributes: None,
            kind: FileKind::Folder,
            ext: String::new(),
            hidden: false,
            type_name: intern(tr!("Локальный диск", "Local Disk")),
        }
    }
}

fn intern(s: &str) -> Arc<str> {
    static TABLE: OnceLock<Mutex<HashMap<String, Arc<str>>>> = OnceLock::new();
    let mut t = TABLE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(a) = t.get(s) {
        return a.clone();
    }
    let a: Arc<str> = Arc::from(s);
    t.insert(s.to_string(), a.clone());
    a
}

/// Explorer-style type description ("Текстовый документ", "Изображение PNG").
pub fn type_name(ext: &str, kind: FileKind, is_dir: bool) -> Arc<str> {
    if is_dir {
        return intern(tr!("Папка с файлами", "File folder"));
    }
    let up = ext.to_uppercase();
    let s = match ext {
        "" => tr!("Файл", "File").into(),
        "md" => "Markdown".into(),
        "pdf" => tr!("Документ PDF", "PDF Document").into(),
        "txt" => tr!("Текстовый документ", "Text Document").into(),
        "log" => tr!("Файл журнала", "Log File").into(),
        "doc" | "docx" | "odt" | "rtf" => tr!("Документ Word", "Word Document").into(),
        "xls" | "xlsx" | "ods" | "csv" => tr!("Электронная таблица", "Spreadsheet").into(),
        "ppt" | "pptx" | "odp" => tr!("Презентация", "Presentation").into(),
        "exe" | "app" | "com" => tr!("Приложение", "Application").into(),
        "dll" | "sys" => tr!("Системный файл", "System File").into(),
        "msi" => tr!("Установщик", "Installer").into(),
        "rs" => tr!("Исходный код Rust", "Rust Source Code").into(),
        "lnk" => tr!("Ярлык", "Shortcut").into(),
        _ => match kind {
            FileKind::Image => trf!("Изображение {up}", "{up} Image"),
            FileKind::Audio => trf!("Аудио {up}", "{up} Audio"),
            FileKind::Video => trf!("Видео {up}", "{up} Video"),
            FileKind::Archive => trf!("Архив {up}", "{up} Archive"),
            FileKind::Code => trf!("Исходный код {up}", "{up} Source Code"),
            _ => trf!("Файл {up}", "{up} File"),
        },
    };
    intern(&s)
}

/// List order: ".." first, then folders, then files; within a group by `key`.
pub fn compare(a: &Entry, b: &Entry, key: SortKey, asc: bool) -> Ordering {
    b.is_parent.cmp(&a.is_parent).then(b.is_dir.cmp(&a.is_dir)).then_with(|| {
        let o = match key {
            SortKey::Name => a.lower.cmp(&b.lower),
            SortKey::Size => a.size.cmp(&b.size).then(a.lower.cmp(&b.lower)),
            SortKey::Modified => a.modified.cmp(&b.modified).then(a.lower.cmp(&b.lower)),
            SortKey::Created => a.created.cmp(&b.created).then(a.lower.cmp(&b.lower)),
            SortKey::Accessed => a.accessed.cmp(&b.accessed).then(a.lower.cmp(&b.lower)),
            SortKey::Extension => a.ext.cmp(&b.ext).then(a.lower.cmp(&b.lower)),
            SortKey::Attributes => a.attributes.cmp(&b.attributes).then(a.lower.cmp(&b.lower)),
            SortKey::Type => a.type_name.cmp(&b.type_name).then(a.lower.cmp(&b.lower)),
        };
        if asc {
            o
        } else {
            o.reverse()
        }
    })
}

pub fn sort(entries: &mut [Entry], key: SortKey, asc: bool) {
    entries.sort_by(|a, b| compare(a, b, key, asc));
}

/// Compiled name filter of a panel (Ctrl+F): substring or regular expression.
pub enum Filter {
    All,
    Substring { text: String, case: bool },
    Regex(regex::Regex),
    /// The expression did not compile: nothing but ".." matches.
    Invalid,
}

impl Filter {
    /// Build a filter; the second value is an error message for an invalid regex.
    pub fn new(raw: &str, regex: bool, case: bool) -> (Filter, Option<String>) {
        let raw = raw.trim();
        if raw.is_empty() {
            (Filter::All, None)
        } else if regex {
            match regex::RegexBuilder::new(raw).case_insensitive(!case).size_limit(1 << 20).build() {
                Ok(re) => (Filter::Regex(re), None),
                Err(err) => (Filter::Invalid, Some(regex_error_text(&err))),
            }
        } else if case {
            (Filter::Substring { text: raw.to_string(), case }, None)
        } else {
            (Filter::Substring { text: raw.to_lowercase(), case }, None)
        }
    }

    pub fn matches(&self, e: &Entry) -> bool {
        match self {
            Filter::All => true,
            _ if e.is_parent => false,
            Filter::Substring { text, case: true } => e.name.contains(text.as_str()),
            Filter::Substring { text, case: false } => e.lower.contains(text.as_str()),
            Filter::Regex(re) => re.is_match(&e.name),
            Filter::Invalid => false,
        }
    }
}

pub fn regex_error_text(err: &regex::Error) -> String {
    match err {
        regex::Error::CompiledTooBig(_) => tr!("Слишком сложное регулярное выражение", "Regular expression is too complex").into(),
        regex::Error::Syntax(msg) => {
            let last = msg.lines().rev().find(|l| l.starts_with("error:")).unwrap_or("").trim_start_matches("error:").trim();
            if last.is_empty() {
                tr!("Ошибка в регулярном выражении", "Error in regular expression").into()
            } else {
                trf!("Ошибка в регулярном выражении: {last}", "Error in regular expression: {last}")
            }
        }
        _ => tr!("Ошибка в регулярном выражении", "Error in regular expression").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> Entry {
        Entry::item(name.into(), name, PathBuf::from(name), Meta { is_dir: false, size, modified: None, hidden: false, ..Meta::default() })
    }

    #[test]
    fn folders_first_then_by_key() {
        let mut v = vec![
            file("b.txt", 1),
            Entry::item("Dir".into(), "Dir", "Dir".into(), Meta { is_dir: true, size: 9, modified: None, hidden: false, ..Meta::default() }),
            file("a.txt", 5),
            Entry::parent(PathBuf::from("/")),
        ];
        sort(&mut v, SortKey::Size, false);
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "Dir", "a.txt", "b.txt"]);
    }

    #[test]
    fn additional_columns_sort_by_metadata_in_both_directions() {
        let mut a = file("a.txt", 0);
        let mut b = file("b.rs", 0);
        a.created = Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(20));
        b.created = Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10));
        a.accessed = a.created;
        b.accessed = b.created;
        a.attributes = Some("RH".into());
        b.attributes = Some("A".into());
        for key in [SortKey::Created, SortKey::Accessed, SortKey::Extension, SortKey::Attributes] {
            assert_eq!(compare(&a, &b, key, true), Ordering::Greater);
            assert_eq!(compare(&a, &b, key, false), Ordering::Less);
            assert_eq!(compare(&Entry::parent("/".into()), &a, key, false), Ordering::Less);
        }
        b.created = None;
        assert_eq!(compare(&a, &b, SortKey::Created, true), Ordering::Greater);
    }

    #[test]
    fn type_names_are_shared() {
        let (a, b) = (file("a.png", 0), file("b.png", 0));
        assert!(Arc::ptr_eq(&a.type_name, &b.type_name));
        assert_eq!(&*a.type_name, "Изображение PNG");
    }

    #[test]
    fn filters() {
        let e = file("Report.TXT", 0);
        assert!(Filter::new("report", false, false).0.matches(&e));
        assert!(!Filter::new("report", false, true).0.matches(&e));
        assert!(Filter::new(r"^rep.*\.txt$", true, false).0.matches(&e));
        let (f, err) = Filter::new("(", true, false);
        assert!(err.is_some() && !f.matches(&e));
        assert!(!Filter::new("x", false, false).0.matches(&Entry::parent("/".into())));
    }
}
