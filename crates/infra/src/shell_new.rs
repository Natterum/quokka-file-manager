//! The Explorer New menu. Let the shell enumerate and execute ShellNew handlers,
//! including application templates, binary data and command-based creators.

#[cfg(windows)]
mod imp {
    use std::{
        collections::HashSet,
        ffi::c_void,
        os::windows::ffi::OsStrExt,
        path::{Path, PathBuf},
        ptr,
    };
    use windows_sys::{
        core::GUID,
        Win32::{
            Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
            System::Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                COINIT_APARTMENTTHREADED,
            },
            UI::{
                Shell::{Common::ITEMIDLIST, *},
                WindowsAndMessaging::*,
            },
        },
    };

    const IID_MENU3: GUID = GUID::from_u128(0xbcfce0a0_ec17_11d0_8d10_00a0c90f2719);
    const IID_INIT: GUID = GUID::from_u128(0x000214e8_0000_0000_c000_000000000046);
    const SUBCLASS: usize = 0x514b4e45;
    const FIRST_ID: u32 = 1;

    #[repr(C)]
    struct UnknownVtbl {
        query: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    #[repr(C)]
    struct MenuVtbl {
        unknown: UnknownVtbl,
        query_menu: unsafe extern "system" fn(*mut c_void, HMENU, u32, u32, u32, u32) -> i32,
        invoke: unsafe extern "system" fn(*mut c_void, *const CMINVOKECOMMANDINFO) -> i32,
        get_command_string: usize,
        handle_msg: usize,
        handle_msg2:
            unsafe extern "system" fn(*mut c_void, u32, WPARAM, LPARAM, *mut LRESULT) -> i32,
    }

    #[repr(C)]
    struct InitVtbl {
        unknown: UnknownVtbl,
        initialize: unsafe extern "system" fn(
            *mut c_void,
            *const ITEMIDLIST,
            *mut c_void,
            *mut c_void,
        ) -> i32,
    }

    struct ComPtr(*mut c_void);
    impl ComPtr {
        unsafe fn vtable<T>(&self) -> &T {
            &**(self.0 as *const *const T)
        }
    }
    impl Drop for ComPtr {
        fn drop(&mut self) {
            unsafe {
                (self.vtable::<UnknownVtbl>().release)(self.0);
            }
        }
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    struct Pidl(*mut ITEMIDLIST);
    impl Drop for Pidl {
        fn drop(&mut self) {
            unsafe {
                ILFree(self.0);
            }
        }
    }
    struct Menu(HMENU);
    impl Drop for Menu {
        fn drop(&mut self) {
            unsafe {
                DestroyMenu(self.0);
            }
        }
    }

    fn check(hr: i32) -> Result<(), String> {
        if hr < 0 {
            Err(format!("Windows Shell: HRESULT 0x{:08X}", hr as u32))
        } else {
            Ok(())
        }
    }

    unsafe fn new_menu(path: &Path) -> Result<(ComPtr, Menu), String> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut pidl = ptr::null_mut();
        check(SHParseDisplayName(
            wide.as_ptr(),
            ptr::null_mut(),
            &mut pidl,
            0,
            ptr::null_mut(),
        ))?;
        let pidl = Pidl(pidl);
        let mut obj = ptr::null_mut();
        check(CoCreateInstance(
            &CLSID_NewMenu,
            ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_MENU3,
            &mut obj,
        ))?;
        let obj = ComPtr(obj);
        let mut init = ptr::null_mut();
        check((obj.vtable::<UnknownVtbl>().query)(
            obj.0, &IID_INIT, &mut init,
        ))?;
        let init = ComPtr(init);
        check((init.vtable::<InitVtbl>().initialize)(
            init.0,
            pidl.0,
            ptr::null_mut(),
            ptr::null_mut(),
        ))?;
        let menu = Menu(CreatePopupMenu());
        if menu.0.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        check((obj.vtable::<MenuVtbl>().query_menu)(
            obj.0, menu.0, 0, FIRST_ID, 0x7fff, CMF_NORMAL,
        ))?;
        Ok((obj, menu))
    }

    unsafe extern "system" fn menu_proc(
        hwnd: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
        _: usize,
        data: usize,
    ) -> LRESULT {
        if matches!(
            msg,
            WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR
        ) {
            let obj = &*(data as *const ComPtr);
            let mut result = 0;
            if (obj.vtable::<MenuVtbl>().handle_msg2)(obj.0, msg, wp, lp, &mut result) == 0 {
                return result;
            }
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }

    struct Subclass(HWND);
    impl Drop for Subclass {
        fn drop(&mut self) {
            unsafe {
                RemoveWindowSubclass(self.0, Some(menu_proc), SUBCLASS);
            }
        }
    }

    unsafe fn invoke(obj: &ComPtr, hwnd: HWND, path: &Path, command: u32) -> Result<(), String> {
        let mut info: CMINVOKECOMMANDINFOEX = std::mem::zeroed();
        let directory: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        info.cbSize = std::mem::size_of_val(&info) as u32;
        info.fMask = 0x00004000; // CMIC_MASK_UNICODE
        info.hwnd = hwnd;
        info.lpVerb = (command - FIRST_ID) as usize as *const u8;
        info.lpVerbW = (command - FIRST_ID) as usize as *const u16;
        info.lpDirectoryW = directory.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        check((obj.vtable::<MenuVtbl>().invoke)(
            obj.0,
            &info as *const _ as *const CMINVOKECOMMANDINFO,
        ))
    }

    unsafe fn invoke_created(
        obj: &ComPtr,
        hwnd: HWND,
        path: &Path,
        command: u32,
    ) -> Result<Option<PathBuf>, String> {
        let entries = || -> Result<HashSet<PathBuf>, String> {
            std::fs::read_dir(path)
                .map_err(|e| e.to_string())?
                .map(|entry| entry.map(|e| e.path()).map_err(|e| e.to_string()))
                .collect()
        };
        let before = entries()?;
        invoke(obj, hwnd, path, command)?;
        let after = entries()?;
        let mut created = after.difference(&before);
        let first = created.next().cloned();
        // Do not offer to rename an arbitrary item if a handler created several.
        Ok(if created.next().is_none() {
            first
        } else {
            None
        })
    }

    /// Show Explorer's New submenu and return the created item for naming.
    /// Cancellation, or a handler with no single new item, returns None.
    /// Must run on the window's UI thread; the native menu pumps Windows messages.
    pub fn show(hwnd: isize, path: &Path) -> Result<Option<PathBuf>, String> {
        if hwnd == 0 || !path.is_dir() {
            return Err("Invalid window or destination folder".into());
        }
        unsafe {
            check(CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32))?;
            let _apartment = Apartment;
            let (obj, menu) = new_menu(path)?;
            let popup = GetSubMenu(menu.0, 0);
            if popup.is_null() {
                return Err("Windows New submenu is unavailable".into());
            }
            let hwnd = hwnd as HWND;
            if SetWindowSubclass(
                hwnd,
                Some(menu_proc),
                SUBCLASS,
                &obj as *const ComPtr as usize,
            ) == 0
            {
                return Err("Cannot attach Windows menu handler".into());
            }
            let subclass = Subclass(hwnd);
            let mut pos = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut pos) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let command = TrackPopupMenuEx(
                popup,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pos.x,
                pos.y,
                hwnd,
                ptr::null(),
            ) as u32;
            drop(subclass);
            if command != 0 {
                return invoke_created(&obj, hwnd, path, command);
            }
            Ok(None)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn windows_new_handler_populates_templates() {
            unsafe {
                check(CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32)).unwrap();
                let _apartment = Apartment;
                let (obj, menu) = new_menu(&std::env::temp_dir()).unwrap();
                let popup = GetSubMenu(menu.0, 0);
                assert!(!popup.is_null());
                let mut result = 0;
                check((obj.vtable::<MenuVtbl>().handle_msg2)(
                    obj.0,
                    WM_INITMENUPOPUP,
                    popup as usize,
                    0,
                    &mut result,
                ))
                .unwrap();
                assert!(
                    GetMenuItemCount(popup) > 0,
                    "New menu must contain creation commands"
                );
            }
        }

        #[test]
        fn installed_office_handlers_create_files_without_overwriting() {
            let path = std::env::temp_dir().join(format!(
                "quokka-shell-new-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            let outcome = (|| -> Result<(), String> {
                unsafe {
                    check(CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32))?;
                    let _apartment = Apartment;
                    let (obj, menu) = new_menu(&path)?;
                    let popup = GetSubMenu(menu.0, 0);
                    assert!(!popup.is_null());
                    let mut result = 0;
                    check((obj.vtable::<MenuVtbl>().handle_msg2)(
                        obj.0,
                        WM_INITMENUPOPUP,
                        popup as usize,
                        0,
                        &mut result,
                    ))?;
                    for index in 0..GetMenuItemCount(popup) {
                        let mut label = [0u16; 512];
                        let len = GetMenuStringW(
                            popup,
                            index as u32,
                            label.as_mut_ptr(),
                            label.len() as i32,
                            MF_BYPOSITION,
                        );
                        let label = String::from_utf16_lossy(&label[..len as usize]);
                        // Office entries are optional, as on machines without Office.
                        if label.contains("Word") || label.contains("Excel") {
                            let command = GetMenuItemID(popup, index);
                            let first = invoke_created(&obj, ptr::null_mut(), &path, command)?
                                .expect("Return the first document for naming");
                            let second = invoke_created(&obj, ptr::null_mut(), &path, command)?
                                .expect("Return the second document for naming");
                            assert_ne!(first, second);
                            assert!(first.is_file() && second.is_file());
                            let files: Vec<_> =
                                std::fs::read_dir(&path).unwrap().flatten().collect();
                            assert_eq!(
                                files.len(),
                                2,
                                "Each creation must produce a separate file: {label}"
                            );
                            assert!(
                                files.iter().all(|file| file.path().extension().is_some()),
                                "The shell must supply the document extension"
                            );
                            for file in files {
                                std::fs::remove_file(file.path()).unwrap();
                            }
                        }
                    }
                }
                Ok(())
            })();
            for file in std::fs::read_dir(&path).unwrap().flatten() {
                std::fs::remove_file(file.path()).unwrap();
            }
            std::fs::remove_dir(&path).unwrap();
            outcome.unwrap();
        }
    }
}

#[cfg(windows)]
pub use imp::show;
