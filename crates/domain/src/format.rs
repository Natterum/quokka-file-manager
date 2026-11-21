//! Human-readable formatting of counts, sizes, dates and binary data.

use std::time::SystemTime;

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

/// "5 элементов" / "5 items"
pub fn items_text(n: u64) -> String {
    if crate::lang::is_en() {
        format!("{n} {}", if n == 1 { "item" } else { "items" })
    } else {
        format!("{n} {}", plural(n, "элемент", "элемента", "элементов"))
    }
}

pub fn format_size(b: u64) -> String {
    let units: [&str; 5] =
        if crate::lang::is_en() { ["KB", "MB", "GB", "TB", "PB"] } else { ["КБ", "МБ", "ГБ", "ТБ", "ПБ"] };
    if b < 1024 {
        return if crate::lang::is_en() { format!("{b} B") } else { format!("{b} Б") };
    }
    let mut v = b as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{:.0} {}", v, units[i])
    } else if v >= 10.0 {
        format!("{:.1} {}", v, units[i])
    } else {
        format!("{:.2} {}", v, units[i])
    }
}

pub fn format_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => chrono::DateTime::<chrono::Local>::from(t).format("%d.%m.%Y  %H:%M").to_string(),
        None => "—".into(),
    }
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
