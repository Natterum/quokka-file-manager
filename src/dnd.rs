//! Drag and drop with the OS: dragging files out of the window into Explorer (or any
//! other app that accepts files) and the bits needed to handle files dropped onto it.
//!
//! Dropping *into* the window is delivered by winit (`RawInput::dropped_files`); during
//! such a drag the window gets no mouse events, so the cursor position and modifier
//! keys are read straight from the OS here.
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

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_SHIFT};
    use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows_sys::Win32::UI::Shell::{CIDLData_CreateFromIDArray, ILFree, SHDoDragDrop, SHParseDisplayName};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    #[repr(C)]
    struct UnknownVtbl {
        query_interface: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
    }

    /// Start a shell drag of `paths` and block until it ends (the OS runs its own
    /// message loop meanwhile). Returns false if nothing could be dragged. The drop
    /// target does the copy or move itself, so the caller only needs to refresh.
    pub fn drag_out(hwnd: isize, paths: &[PathBuf]) -> bool {
        unsafe {
            let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
            for p in paths {
                let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
                let mut pidl = std::ptr::null_mut();
                if SHParseDisplayName(wide.as_ptr(), std::ptr::null_mut(), &mut pidl, 0, std::ptr::null_mut()) >= 0
                    && !pidl.is_null()
                {
                    pidls.push(pidl);
                }
            }
            let mut ok = false;
            if !pidls.is_empty() {
                // absolute ID lists are relative to the desktop, whose ID list is empty
                let desktop = [0u8; 2];
                let children: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
                let mut data = std::ptr::null_mut();
                let hr = CIDLData_CreateFromIDArray(
                    desktop.as_ptr() as *const ITEMIDLIST,
                    children.len() as u32,
                    children.as_ptr(),
                    &mut data,
                );
                if hr >= 0 && !data.is_null() {
                    let allowed: DROPEFFECT = DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK;
                    let mut effect: DROPEFFECT = 0;
                    // no IDropSource: the shell supplies the standard one (Esc cancels,
                    // Ctrl / Shift / Alt pick copy / move / shortcut)
                    ok = SHDoDragDrop(hwnd as HWND, data, std::ptr::null_mut(), allowed, &mut effect) >= 0;
                    let vtbl = *(data as *const *const UnknownVtbl);
                    ((*vtbl).release)(data);
                }
            }
            for p in pidls {
                ILFree(p);
            }
            ok
        }
    }

    /// Cursor position in the window's client area, in physical pixels.
    pub fn cursor_in_client(hwnd: isize) -> Option<(f32, f32)> {
        unsafe {
            let mut pt = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut pt) == 0 || ScreenToClient(hwnd as HWND, &mut pt) == 0 {
                return None;
            }
            Some((pt.x as f32, pt.y as f32))
        }
    }

    /// (Ctrl, Shift) held right now, regardless of which window has focus.
    pub fn keys() -> (bool, bool) {
        unsafe {
            let down = |vk: u16| (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0;
            (down(VK_CONTROL), down(VK_SHIFT))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;
    pub fn drag_out(_hwnd: isize, _paths: &[PathBuf]) -> bool {
        false
    }
    pub fn cursor_in_client(_hwnd: isize) -> Option<(f32, f32)> {
        None
    }
    pub fn keys() -> (bool, bool) {
        (false, false)
    }
}

pub use imp::{cursor_in_client, drag_out, keys};

/// Native window handle (HWND on Windows), 0 when unavailable.
pub fn window_handle(frame: &eframe::Frame) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().map(|h| h.as_raw()) {
        #[cfg(windows)]
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
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
