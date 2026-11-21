//! The Windows "Properties" dialog for files, folders and drives — the same sheet
//! Explorer shows (general info, security, previous versions, details …). Several
//! items open one combined sheet, as in Explorer.

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::S_FALSE;
    use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows_sys::Win32::UI::Shell::{CIDLData_CreateFromIDArray, ILFree, SHMultiFileProperties, SHParseDisplayName};

    #[repr(C)]
    struct UnknownVtbl {
        query_interface: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
    }

    /// Frees the parsed ID lists however `show` returns.
    struct Pidls(Vec<*mut ITEMIDLIST>);

    impl Drop for Pidls {
        fn drop(&mut self) {
            for p in self.0.drain(..) {
                unsafe { ILFree(p) };
            }
        }
    }

    fn parse(path: &Path) -> Result<*mut ITEMIDLIST, String> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut pidl = std::ptr::null_mut();
        let hr = unsafe { SHParseDisplayName(wide.as_ptr(), std::ptr::null_mut(), &mut pidl, 0, std::ptr::null_mut()) };
        if hr < 0 || pidl.is_null() {
            return Err(format!("{}: 0x{:08X}", path.display(), hr as u32));
        }
        Ok(pidl)
    }

    /// Open the shell's property sheet for `paths`. The sheet runs on its own shell
    /// thread, so this returns at once and the window stays responsive.
    pub fn show(paths: &[PathBuf]) -> Result<(), String> {
        if paths.is_empty() {
            return Err("nothing selected".into());
        }
        // The UI thread already has OLE (drag and drop), so this is normally a no-op.
        // On a thread without COM it stays initialized: the sheet keeps using it.
        let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if hr == S_FALSE {
            unsafe { CoUninitialize() };
        }
        let mut pidls = Pidls(Vec::with_capacity(paths.len()));
        for p in paths {
            pidls.0.push(parse(p)?);
        }
        unsafe {
            // absolute ID lists are children of the desktop, whose ID list is empty
            let desktop = [0u8; 2];
            let children: Vec<*const ITEMIDLIST> = pidls.0.iter().map(|p| *p as *const _).collect();
            let mut data = std::ptr::null_mut();
            let hr = CIDLData_CreateFromIDArray(desktop.as_ptr() as *const ITEMIDLIST, children.len() as u32, children.as_ptr(), &mut data);
            if hr < 0 || data.is_null() {
                return Err(format!("CIDLData_CreateFromIDArray: 0x{:08X}", hr as u32));
            }
            let hr = SHMultiFileProperties(data, 0);
            let vtbl = *(data as *const *const UnknownVtbl);
            ((*vtbl).release)(data);
            if hr < 0 {
                return Err(format!("SHMultiFileProperties: 0x{:08X}", hr as u32));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PeekMessageW,
            PostMessageW, TranslateMessage, MSG, PM_REMOVE, WM_CLOSE,
        };

        /// The sheet calls back into the data object, which lives on this thread:
        /// pump messages like the UI thread does.
        fn pump(ms: u64) {
            let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < end {
                unsafe {
                    let mut msg: MSG = std::mem::zeroed();
                    while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        unsafe extern "system" fn collect(hwnd: HWND, out: LPARAM) -> BOOL {
            let out = &mut *(out as *mut Vec<(HWND, String)>);
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == std::process::id() && IsWindowVisible(hwnd) != 0 {
                let mut buf = [0u16; 256];
                let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
                out.push((hwnd, String::from_utf16_lossy(&buf[..n.max(0) as usize])));
            }
            1
        }

        fn own_windows() -> Vec<(HWND, String)> {
            let mut out: Vec<(HWND, String)> = Vec::new();
            unsafe { EnumWindows(Some(collect), &mut out as *mut _ as LPARAM) };
            out
        }

        #[test]
        fn missing_path_is_an_error() {
            assert!(show(&[PathBuf::from(r"C:\definitely\not\here\quokka.txt")]).is_err());
            assert!(show(&[]).is_err());
        }

        /// Opens a real property sheet; needs an interactive desktop.
        #[test]
        #[ignore]
        fn property_sheet_opens_for_a_file() {
            let dir = std::env::temp_dir().join("quokka-props-test");
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("sample.txt");
            std::fs::write(&file, "x").unwrap();
            show(&[file.clone(), dir.clone()]).unwrap();
            let mut found = None;
            for _ in 0..50 {
                pump(100);
                found = own_windows().into_iter().find(|(_, t)| !t.is_empty());
                if found.is_some() {
                    break;
                }
            }
            let (hwnd, title) = found.expect("no property sheet appeared");
            eprintln!("property sheet: {title}");
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            pump(300);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;
    pub fn show(_paths: &[PathBuf]) -> Result<(), String> {
        Err("not supported on this system".into())
    }
}

pub use imp::show;
