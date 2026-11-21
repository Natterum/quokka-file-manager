use crate::theme::*;
use eframe::egui::Color32;
use std::time::SystemTime;

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

impl FileKind {
    pub fn from_name(name: &str) -> Self {
        let ext = match name.rsplit_once('.') {
            Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
            _ => return FileKind::Other,
        };
        match ext.as_str() {
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

    pub fn color(self) -> Color32 {
        match self {
            FileKind::Folder => YELLOW,
            FileKind::Document => BLUE,
            FileKind::Image => TEAL,
            FileKind::Video => PURPLE,
            FileKind::Audio => ORANGE,
            FileKind::Archive => RED,
            FileKind::Code => GREEN,
            FileKind::Executable => SLATE,
            FileKind::Other => GRAY,
        }
    }


}

/// Russian plural form: 1 файл, 2 файла, 5 файлов.
pub fn plural<'a>(n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let (m10, m100) = (n % 10, n % 100);
    if m10 == 1 && m100 != 11 {
        one
    } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
        few
    } else {
        many
    }
}

/// "5 элементов"
pub fn items_text(n: u64) -> String {
    format!("{n} {}", plural(n, "элемент", "элемента", "элементов"))
}

pub fn format_size(b: u64) -> String {
    const UNITS: [&str; 5] = ["КБ", "МБ", "ГБ", "ТБ", "ПБ"];
    if b < 1024 {
        return format!("{b} Б");
    }
    let mut v = b as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{:.0} {}", v, UNITS[i])
    } else if v >= 10.0 {
        format!("{:.1} {}", v, UNITS[i])
    } else {
        format!("{:.2} {}", v, UNITS[i])
    }
}

pub fn format_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => chrono::DateTime::<chrono::Local>::from(t).format("%d.%m.%Y  %H:%M").to_string(),
        None => "—".into(),
    }
}

/// Linear blend: t = 0 → a, t = 1 → b.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn hex_dump(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 4);
    for (row, chunk) in data.chunks(16).enumerate() {
        out.push_str(&format!("{:08X}  ", row * 16));
        for i in 0..16 {
            match chunk.get(i) {
                Some(b) => out.push_str(&format!("{b:02X} ")),
                None => out.push_str("   "),
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        for &b in chunk {
            out.push(if (0x20..0x7f).contains(&b) { b as char } else { '.' });
        }
        out.push('\n');
    }
    out
}
