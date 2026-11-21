//! Drag and drop with the Windows shell: dragging files out of the window into
//! Explorer (or any other app that accepts files), and reading the cursor position
//! and modifier keys during a drag that started in another program — the window gets
//! no mouse events then.

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
