//! File categories derived from the extension.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum FileKind {
    Folder,
    Document,
    Image,
    Video,
    Audio,
    Archive,
    Code,
    Executable,
    Other,
}

/// Lower-case extension of a file name; empty for names without one and for
/// dot-files such as `.gitignore`.
pub fn ext_of(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

impl FileKind {
    pub fn from_name(name: &str) -> Self {
        Self::from_ext(&ext_of(name))
    }

    pub fn from_ext(ext: &str) -> Self {
        match ext {
            "" => FileKind::Other,
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tif" | "tiff" | "heic" | "psd"
            | "ai" | "raw" | "avif" => FileKind::Image,
            "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "wma" | "opus" | "mid" => FileKind::Audio,
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "flv" | "m4v" | "mpg" | "mpeg" => FileKind::Video,
            "txt" | "md" | "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp"
            | "rtf" | "csv" | "epub" | "log" | "djvu" => FileKind::Document,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "iso" | "cab" | "zst" | "tgz" => FileKind::Archive,
            "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "cpp" | "h" | "hpp" | "cs" | "java" | "go"
            | "html" | "htm" | "css" | "scss" | "json" | "toml" | "yaml" | "yml" | "xml" | "sh" | "ps1" | "bat"
            | "cmd" | "lua" | "kt" | "swift" | "sql" | "ini" | "cfg" | "vue" | "php" | "rb" | "lock" => FileKind::Code,
            "exe" | "msi" | "dll" | "app" | "dmg" | "deb" | "rpm" | "apk" | "com" | "sys" => FileKind::Executable,
            _ => FileKind::Other,
        }
    }
}
