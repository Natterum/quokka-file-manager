//! Native Windows «Open file» dialogs.

use quokka_domain::tr;
use std::path::{Path, PathBuf};

/// Pick a private key file, starting in `start` (or `~/.ssh`). `owner` — the
/// window the dialog belongs to (0 for none).
#[cfg(windows)]
pub fn key_file(owner: isize, start: Option<&Path>) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let filter: Vec<u16> = tr!(
        "Все файлы\0*.*\0Ключи OpenSSH (id_*)\0id_*\0Ключи PEM\0*.pem;*.key\0\0",
        "All files\0*.*\0OpenSSH keys (id_*)\0id_*\0PEM keys\0*.pem;*.key\0\0"
    )
    .encode_utf16()
    .collect();
    let title: Vec<u16> = tr!("Выберите файл закрытого ключа\0", "Choose the private key file\0").encode_utf16().collect();
    let ssh = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".ssh"));
    let dir = start
        .and_then(|p| if p.is_dir() { Some(p.to_path_buf()) } else { p.parent().map(Path::to_path_buf) })
        .filter(|d| d.is_dir())
        .or(ssh.filter(|d| d.is_dir()));
    let dir_w: Option<Vec<u16>> = dir.map(|d| d.to_string_lossy().encode_utf16().chain(Some(0)).collect());
    let mut buf = vec![0u16; 4096];
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner as _;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    if let Some(d) = &dir_w {
        ofn.lpstrInitialDir = d.as_ptr();
    }
    ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER | OFN_HIDEREADONLY;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

#[cfg(not(windows))]
pub fn key_file(_owner: isize, _start: Option<&Path>) -> Option<PathBuf> {
    None
}

/// Choose a Quokka settings file to import.
pub fn settings_open(owner: isize) -> Option<PathBuf> {
    settings_dialog(owner, false)
}

/// Choose where to export the settings; `.txt` is added when no extension is typed.
pub fn settings_save(owner: isize) -> Option<PathBuf> {
    settings_dialog(owner, true)
}

#[cfg(windows)]
fn settings_dialog(owner: isize, save: bool) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let filter: Vec<u16> = tr!(
        "Настройки Quokka (*.txt)\0*.txt\0Все файлы\0*.*\0\0",
        "Quokka settings (*.txt)\0*.txt\0All files\0*.*\0\0"
    )
    .encode_utf16()
    .collect();
    let title = if save {
        tr!("Экспорт настроек Quokka\0", "Export Quokka settings\0")
    } else {
        tr!("Импорт настроек Quokka\0", "Import Quokka settings\0")
    };
    let title: Vec<u16> = title.encode_utf16().collect();
    let def_ext: Vec<u16> = "txt\0".encode_utf16().collect();
    let mut buf = vec![0u16; 4096];
    if save {
        for (i, c) in "quokka-settings.txt".encode_utf16().enumerate() {
            buf[i] = c;
        }
    }
    let docs = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Documents")).filter(|d| d.is_dir());
    let dir_w: Option<Vec<u16>> = docs.map(|d| d.to_string_lossy().encode_utf16().chain(Some(0)).collect());
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner as _;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.lpstrDefExt = def_ext.as_ptr();
    if let Some(d) = &dir_w {
        ofn.lpstrInitialDir = d.as_ptr();
    }
    let ok = unsafe {
        if save {
            ofn.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER | OFN_HIDEREADONLY;
            GetSaveFileNameW(&mut ofn)
        } else {
            ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER | OFN_HIDEREADONLY;
            GetOpenFileNameW(&mut ofn)
        }
    };
    if ok == 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

#[cfg(not(windows))]
fn settings_dialog(_owner: isize, _save: bool) -> Option<PathBuf> {
    None
}
