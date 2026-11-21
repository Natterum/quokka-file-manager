//! Custom sidebar icons: Windows icon resources (exe/dll/ico), a folder's own shell icon,
//! and image files (png/jpg/bmp/gif/webp/svg). Everything ends up as RGBA pixels.
//!
//! An icon is described by a short spec string kept in the settings:
//! * `shell`              — the Windows icon of the bookmarked folder itself
//! * `C:\…\shell32.dll,4`  — icon #4 from a resource file (exe, dll, ico, cpl, …)
//! * `C:\…\picture.png`    — an image file

use std::path::{Path, PathBuf};

pub const SHELL: &str = "shell";

/// Square RGBA image (not premultiplied).
pub struct Rgba {
    pub size: usize,
    pub pixels: Vec<u8>,
}

/// System libraries offered in the "Windows" tab of the icon picker.
pub const WINDOWS_LIBS: [(&str, &str); 7] = [
    ("Основные (imageres.dll)", "imageres.dll"),
    ("Классические (shell32.dll)", "shell32.dll"),
    ("Устройства (ddores.dll)", "ddores.dll"),
    ("Мультимедиа (mmres.dll)", "mmres.dll"),
    ("Проводник (explorer.exe)", "explorer.exe"),
    ("Старые (moricons.dll)", "moricons.dll"),
    ("Сеть (netshell.dll)", "netshell.dll"),
];

pub fn system_lib(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    if name.ends_with(".exe") && name != "explorer.exe" {
        root.join("System32").join(name)
    } else if name == "explorer.exe" {
        root.join(name)
    } else {
        root.join("System32").join(name)
    }
}

const RESOURCE_EXTS: [&str; 7] = ["exe", "dll", "ico", "icl", "cpl", "ocx", "mun"];

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// Whether a file holds Windows icon resources (and may hold many of them).
pub fn is_resource_file(p: &Path) -> bool {
    RESOURCE_EXTS.contains(&ext_of(p).as_str())
}

/// Split `file,index` into its parts; a plain path means index 0.
pub fn split_spec(spec: &str) -> (PathBuf, i32) {
    if let Some((file, idx)) = spec.rsplit_once(',') {
        if let Ok(i) = idx.trim().parse::<i32>() {
            return (PathBuf::from(file), i);
        }
    }
    (PathBuf::from(spec), 0)
}

pub fn make_spec(file: &Path, index: i32) -> String {
    format!("{},{}", file.display(), index)
}

/// Short human description of a spec for the UI.
pub fn describe(spec: &str) -> String {
    if spec == SHELL {
        return "Значок папки из Windows".into();
    }
    let (file, idx) = split_spec(spec);
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| spec.to_string());
    if is_resource_file(&file) && ext_of(&file) != "ico" {
        format!("{name}, значок №{idx}")
    } else {
        name
    }
}

/// Load the icon described by `spec`; `folder` is the bookmarked folder (for `shell`).
pub fn load(spec: &str, folder: &Path, size: u32) -> Result<Rgba, String> {
    if spec == SHELL {
        return shell_icon(folder, size).ok_or_else(|| "Windows не вернула значок папки".into());
    }
    let (file, idx) = split_spec(spec);
    if is_resource_file(&file) {
        return extract(&file, idx, size).ok_or_else(|| format!("Не удалось извлечь значок №{idx} из {}", file.display()));
    }
    load_image(&file, size)
}

fn square(img: image::RgbaImage, size: u32) -> Rgba {
    let (w, h) = img.dimensions();
    let scale = size as f32 / w.max(h).max(1) as f32;
    let (nw, nh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
    let resized = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3);
    let mut canvas = image::RgbaImage::new(size, size);
    image::imageops::overlay(&mut canvas, &resized, ((size - nw) / 2) as i64, ((size - nh) / 2) as i64);
    Rgba { size: size as usize, pixels: canvas.into_raw() }
}

fn load_image(file: &Path, size: u32) -> Result<Rgba, String> {
    if ext_of(file) == "svg" {
        use resvg::{tiny_skia, usvg};
        let data = std::fs::read(file).map_err(|e| e.to_string())?;
        let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).map_err(|e| e.to_string())?;
        let sz = tree.size();
        let scale = size as f32 / sz.width().max(sz.height());
        let tx = (size as f32 - sz.width() * scale) / 2.0;
        let ty = (size as f32 - sz.height() * scale) / 2.0;
        let mut pm = tiny_skia::Pixmap::new(size, size).ok_or("пустое изображение")?;
        resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, tx, ty), &mut pm.as_mut());
        let pixels = pm
            .pixels()
            .iter()
            .flat_map(|c| {
                let c = c.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        return Ok(Rgba { size: size as usize, pixels });
    }
    let img = image::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(square(img.to_rgba8(), size))
}

// ───────────────────────────── Windows ─────────────────────────────

#[cfg(windows)]
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Convert an HICON into RGBA pixels and destroy it.
#[cfg(windows)]
unsafe fn hicon_to_rgba(hicon: windows_sys::Win32::UI::WindowsAndMessaging::HICON, size: u32) -> Option<Rgba> {
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};
    if hicon.is_null() {
        return None;
    }
    let mut ii: ICONINFO = std::mem::zeroed();
    if GetIconInfo(hicon, &mut ii) == 0 {
        DestroyIcon(hicon);
        return None;
    }
    let read = |bmp: HBITMAP| -> Option<(usize, usize, Vec<u8>)> {
        if bmp.is_null() {
            return None;
        }
        let mut bm: BITMAP = std::mem::zeroed();
        if GetObjectW(bmp as _, std::mem::size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut _) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth.max(1) as usize, bm.bmHeight.abs().max(1) as usize);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w as i32;
        bi.bmiHeader.biHeight = -(h as i32);
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB as _;
        let mut buf = vec![0u8; w * h * 4];
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let lines = GetDIBits(dc, bmp, 0, h as u32, buf.as_mut_ptr() as *mut _, &mut bi, DIB_RGB_COLORS);
        DeleteDC(dc);
        (lines > 0).then_some((w, h, buf))
    };
    let color = read(ii.hbmColor);
    let mask = read(ii.hbmMask);
    if !ii.hbmColor.is_null() {
        DeleteObject(ii.hbmColor as _);
    }
    if !ii.hbmMask.is_null() {
        DeleteObject(ii.hbmMask as _);
    }
    DestroyIcon(hicon);

    let (w, h, mut px) = color?;
    // BGRA → RGBA
    for c in px.chunks_exact_mut(4) {
        c.swap(0, 2);
    }
    // old icons carry no alpha channel: take transparency from the AND mask
    if px.chunks_exact(4).all(|c| c[3] == 0) {
        match mask {
            Some((mw, _, m)) if mw == w => {
                for (i, c) in px.chunks_exact_mut(4).enumerate() {
                    c[3] = if m[i * 4] == 0 { 255 } else { 0 };
                }
            }
            _ => px.chunks_exact_mut(4).for_each(|c| c[3] = 255),
        }
    }
    let img = image::RgbaImage::from_raw(w as u32, h as u32, px)?;
    Some(square(img, size))
}

/// Icon #`index` from an exe/dll/ico file at roughly `size` pixels.
#[cfg(windows)]
pub fn extract(file: &Path, index: i32, size: u32) -> Option<Rgba> {
    use windows_sys::Win32::UI::WindowsAndMessaging::PrivateExtractIconsW;
    let w = wide(file.as_os_str());
    let mut hicon = std::ptr::null_mut();
    let mut id = 0u32;
    let got = unsafe { PrivateExtractIconsW(w.as_ptr(), index, size as i32, size as i32, &mut hicon, &mut id, 1, 0) };
    if got == 0 || got == u32::MAX {
        return None;
    }
    unsafe { hicon_to_rgba(hicon, size) }
}

/// Number of icons inside a resource file.
#[cfg(windows)]
pub fn count(file: &Path) -> usize {
    use windows_sys::Win32::UI::Shell::ExtractIconExW;
    let w = wide(file.as_os_str());
    let n = unsafe { ExtractIconExW(w.as_ptr(), -1, std::ptr::null_mut(), std::ptr::null_mut(), 0) };
    if n == u32::MAX {
        0
    } else {
        n as usize
    }
}

/// The icon Windows shows for this folder (custom desktop.ini icons included).
#[cfg(windows)]
pub fn shell_icon(folder: &Path, size: u32) -> Option<Rgba> {
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_ICONLOCATION, SHGFI_LARGEICON};
    let path: PathBuf = if folder.as_os_str().is_empty() {
        // "Этот компьютер" has no path; use the computer icon from imageres
        return extract(&system_lib("imageres.dll"), 104, size);
    } else {
        folder.to_path_buf()
    };
    let w = wide(path.as_os_str());
    unsafe {
        // prefer the icon location: lets us extract at full size
        let mut info: SHFILEINFOW = std::mem::zeroed();
        if SHGetFileInfoW(w.as_ptr(), 0, &mut info, std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_ICONLOCATION) != 0 {
            let len = info.szDisplayName.iter().position(|&c| c == 0).unwrap_or(0);
            if len > 0 {
                let file = PathBuf::from(String::from_utf16_lossy(&info.szDisplayName[..len]));
                if let Some(img) = extract(&file, info.iIcon, size) {
                    return Some(img);
                }
            }
        }
        let mut info: SHFILEINFOW = std::mem::zeroed();
        if SHGetFileInfoW(w.as_ptr(), 0, &mut info, std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON) == 0 {
            return None;
        }
        hicon_to_rgba(info.hIcon, size)
    }
}

/// Native "Open file" dialog for picking an icon source.
#[cfg(windows)]
pub fn pick_file() -> Option<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let filter: Vec<u16> = "Значки и изображения\0*.ico;*.png;*.jpg;*.jpeg;*.svg;*.bmp;*.gif;*.webp;*.exe;*.dll;*.icl;*.cpl\0Все файлы\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "Выберите значок\0".encode_utf16().collect();
    let mut buf = vec![0u16; 4096];
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

#[cfg(not(windows))]
pub fn extract(_file: &Path, _index: i32, _size: u32) -> Option<Rgba> {
    None
}
#[cfg(not(windows))]
pub fn count(_file: &Path) -> usize {
    0
}
#[cfg(not(windows))]
pub fn shell_icon(_folder: &Path, _size: u32) -> Option<Rgba> {
    None
}
#[cfg(not(windows))]
pub fn pick_file() -> Option<PathBuf> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn extracts_system_icons() {
        let lib = system_lib("imageres.dll");
        let n = count(&lib);
        assert!(n > 50, "imageres.dll icons: {n}");
        let img = extract(&lib, 3, 32).expect("icon 3");
        assert_eq!(img.size, 32);
        assert!(img.pixels.chunks(4).any(|c| c[3] > 0), "icon is fully transparent");
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap();
        assert!(shell_icon(&home, 64).is_some(), "shell icon of the home folder");
        assert!(load(SHELL, &home, 48).is_ok());
        assert!(load(&make_spec(&system_lib("shell32.dll"), 4), Path::new(""), 32).is_ok());
    }
}
