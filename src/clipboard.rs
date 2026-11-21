//! Native file clipboard, shared with Windows Explorer (CF_HDROP).
use std::path::PathBuf;

#[derive(Debug, PartialEq)]
pub struct Files {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Foundation::{GlobalFree, HGLOBAL, HWND};
    use windows_sys::Win32::System::DataExchange::*;
    use windows_sys::Win32::System::Memory::*;
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::{DragQueryFileW, DROPFILES};

    struct Clipboard;
    impl Clipboard {
        fn open(hwnd: isize) -> Result<Self, String> {
            if unsafe { OpenClipboard(hwnd as HWND) } == 0 {
                return Err("Буфер обмена занят другой программой. Попробуйте ещё раз".into());
            }
            Ok(Self)
        }
    }
    impl Drop for Clipboard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    // Until SetClipboardData succeeds, the allocation belongs to us.
    struct Memory(HGLOBAL);
    impl Memory {
        fn new(bytes: &[u8]) -> Result<Self, String> {
            unsafe {
                let mem = Self(GlobalAlloc(GHND, bytes.len()));
                if mem.0.is_null() {
                    return Err("Недостаточно памяти для буфера обмена".into());
                }
                let ptr = GlobalLock(mem.0);
                if ptr.is_null() {
                    return Err("Не удалось открыть память буфера обмена".into());
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast(), bytes.len());
                GlobalUnlock(mem.0);
                Ok(mem)
            }
        }
        fn publish(mut self, format: u32) -> Result<(), String> {
            if unsafe { SetClipboardData(format, self.0) }.is_null() {
                return Err(format!(
                    "Не удалось записать файлы в буфер обмена: {} (формат {format})",
                    std::io::Error::last_os_error()
                ));
            }
            self.0 = std::ptr::null_mut();
            Ok(())
        }
    }
    impl Drop for Memory {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    GlobalFree(self.0);
                }
            }
        }
    }

    fn effect_format() -> u32 {
        let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    }

    pub fn write(hwnd: isize, files: &Files) -> Result<(), String> {
        let header = std::mem::size_of::<DROPFILES>();
        let mut bytes = vec![0u8; header];
        bytes[..4].copy_from_slice(&(header as u32).to_le_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes()); // fWide
        for path in &files.paths {
            for unit in path.as_os_str().encode_wide().chain(Some(0)) {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
        let data = Memory::new(&bytes)?;
        let effect = Memory::new(&(if files.cut { 2u32 } else { 1u32 }).to_le_bytes())?;
        let format = effect_format();
        if format == 0 {
            return Err("Не удалось зарегистрировать формат буфера обмена".into());
        }
        let _clipboard = Clipboard::open(hwnd)?;
        if unsafe { EmptyClipboard() } == 0 {
            return Err("Не удалось очистить буфер обмена".into());
        }
        data.publish(CF_HDROP as u32)?;
        effect.publish(format)
    }

    pub fn read(hwnd: isize) -> Result<Files, String> {
        let _clipboard = Clipboard::open(hwnd)?;
        unsafe {
            let drop = GetClipboardData(CF_HDROP as u32);
            if drop.is_null() {
                return Err("В буфере обмена нет файлов".into());
            }
            let count = DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0);
            let mut paths = Vec::new();
            for i in 0..count {
                let len = DragQueryFileW(drop, i, std::ptr::null_mut(), 0);
                let mut wide = vec![0u16; len as usize + 1];
                if DragQueryFileW(drop, i, wide.as_mut_ptr(), len + 1) != len || len == 0 {
                    return Err("Не удалось прочитать путь из буфера обмена".into());
                }
                paths.push(PathBuf::from(std::ffi::OsString::from_wide(
                    &wide[..len as usize],
                )));
            }
            if paths.is_empty() {
                return Err("В буфере обмена нет файлов".into());
            }
            let effect = GetClipboardData(effect_format());
            let mut cut = false;
            if !effect.is_null() && GlobalSize(effect) >= 4 {
                let ptr = GlobalLock(effect);
                if !ptr.is_null() {
                    cut = std::ptr::read_unaligned(ptr.cast::<u32>()) & 2 != 0;
                    GlobalUnlock(effect);
                }
            }
            Ok(Files { paths, cut })
        }
    }

    pub fn paste_down() -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL};
        unsafe { GetAsyncKeyState(VK_CONTROL as i32) < 0 && GetAsyncKeyState(b'V' as i32) < 0 }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        #[ignore = "writes to the system clipboard; run explicitly"]
        fn native_file_clipboard_round_trip() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow};
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null());
            for cut in [false, true] {
                let files = Files {
                    paths: vec![
                        PathBuf::from(r"C:\тест\file.txt"),
                        PathBuf::from(r"C:\folder with spaces"),
                    ],
                    cut,
                };
                let result = write(hwnd as isize, &files);
                assert!(result.is_ok(), "{result:?}");
                assert_eq!(read(hwnd as isize).unwrap(), files);
            }
            {
                let _clipboard = Clipboard::open(hwnd as isize).unwrap();
                assert_ne!(unsafe { EmptyClipboard() }, 0);
            }
            unsafe {
                DestroyWindow(hwnd);
            }
        }
    }
}

#[cfg(windows)]
pub use native::{paste_down, read, write};

#[cfg(not(windows))]
pub fn paste_down() -> bool {
    false
}
#[cfg(not(windows))]
pub fn write(_: isize, _: &Files) -> Result<(), String> {
    Err("Файловый буфер обмена доступен только в Windows".into())
}
#[cfg(not(windows))]
pub fn read(_: isize) -> Result<Files, String> {
    Err("Файловый буфер обмена доступен только в Windows".into())
}
