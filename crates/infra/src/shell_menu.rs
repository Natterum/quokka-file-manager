//! The Explorer context menu ("Show more options" in Windows 11): every verb the
//! shell and installed programs register for files, folders, drives or the empty
//! area of a folder — Open with, Send to, Share, archivers, antivirus scans …

#[cfg(windows)]
mod imp {
    use std::{cell::RefCell, ffi::c_void, os::windows::ffi::OsStrExt, path::{Path, PathBuf}, ptr};
    use windows_sys::{
        core::GUID,
        Win32::{
            Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
            System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            UI::{
                Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT},
                Shell::{Common::ITEMIDLIST, *},
                WindowsAndMessaging::*,
            },
        },
    };

    const IID_SHELL_FOLDER: GUID = GUID::from_u128(0x000214e6_0000_0000_c000_000000000046);
    const IID_MENU: GUID = GUID::from_u128(0x000214e4_0000_0000_c000_000000000046);
    const IID_MENU2: GUID = GUID::from_u128(0x000214f4_0000_0000_c000_000000000046);
    const IID_MENU3: GUID = GUID::from_u128(0xbcfce0a0_ec17_11d0_8d10_00a0c90f2719);
    const SUBCLASS: usize = 0x514b4d4e;
    const FIRST_ID: u32 = 1;
    const LAST_ID: u32 = 0x7fff;

    #[repr(C)]
    struct UnknownVtbl {
        query: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    #[repr(C)]
    struct FolderVtbl {
        unknown: UnknownVtbl,
        parse_display_name: usize,
        enum_objects: usize,
        bind_to_object: usize,
        bind_to_storage: usize,
        compare_ids: usize,
        create_view_object: unsafe extern "system" fn(*mut c_void, HWND, *const GUID, *mut *mut c_void) -> i32,
        get_attributes_of: usize,
        get_ui_object_of: unsafe extern "system" fn(
            *mut c_void,
            HWND,
            u32,
            *const *const ITEMIDLIST,
            *const GUID,
            *mut u32,
            *mut *mut c_void,
        ) -> i32,
    }

    #[repr(C)]
    struct MenuVtbl {
        unknown: UnknownVtbl,
        query_menu: unsafe extern "system" fn(*mut c_void, HMENU, u32, u32, u32, u32) -> i32,
        invoke: unsafe extern "system" fn(*mut c_void, *const CMINVOKECOMMANDINFO) -> i32,
        get_command_string: unsafe extern "system" fn(*mut c_void, usize, u32, *mut u32, *mut u8, u32) -> i32,
        /// IContextMenu2
        handle_msg: unsafe extern "system" fn(*mut c_void, u32, WPARAM, LPARAM) -> i32,
        /// IContextMenu3
        handle_msg2: unsafe extern "system" fn(*mut c_void, u32, WPARAM, LPARAM, *mut LRESULT) -> i32,
    }

    struct ComPtr(*mut c_void);
    impl ComPtr {
        unsafe fn vtable<T>(&self) -> &T {
            &**(self.0 as *const *const T)
        }
        unsafe fn query(&self, iid: &GUID) -> Option<ComPtr> {
            let mut out = ptr::null_mut();
            ((self.vtable::<UnknownVtbl>().query)(self.0, iid, &mut out) >= 0 && !out.is_null()).then(|| ComPtr(out))
        }
    }
    impl Drop for ComPtr {
        fn drop(&mut self) {
            unsafe {
                (self.vtable::<UnknownVtbl>().release)(self.0);
            }
        }
    }
    struct Apartment(bool);
    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }
    struct Pidl(*mut ITEMIDLIST);
    impl Drop for Pidl {
        fn drop(&mut self) {
            unsafe { ILFree(self.0) };
        }
    }
    struct Menu(HMENU);
    impl Drop for Menu {
        fn drop(&mut self) {
            unsafe { DestroyMenu(self.0) };
        }
    }

    thread_local! {
        /// Some verbs finish their work after `InvokeCommand` returns (property
        /// sheets, "Open with" …) and expect the menu object to stay alive a while:
        /// keep the last one until the next menu is shown.
        static LAST: RefCell<Option<ComPtr>> = const { RefCell::new(None) };
    }

    fn check(hr: i32) -> Result<(), String> {
        if hr < 0 {
            Err(format!("Windows Shell: HRESULT 0x{:08X}", hr as u32))
        } else {
            Ok(())
        }
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    unsafe fn parse(path: &Path) -> Result<Pidl, String> {
        let w = wide(path);
        let mut pidl = ptr::null_mut();
        let hr = SHParseDisplayName(w.as_ptr(), ptr::null_mut(), &mut pidl, 0, ptr::null_mut());
        if hr < 0 || pidl.is_null() {
            return Err(format!("{}: HRESULT 0x{:08X}", path.display(), hr as u32));
        }
        Ok(Pidl(pidl))
    }

    /// The parent folder of `pidl` and the item's ID relative to it (points into `pidl`).
    unsafe fn bind_parent(pidl: &Pidl) -> Result<(ComPtr, *const ITEMIDLIST), String> {
        let mut folder = ptr::null_mut();
        let mut last = ptr::null_mut();
        check(SHBindToParent(pidl.0, &IID_SHELL_FOLDER, &mut folder, &mut last))?;
        Ok((ComPtr(folder), last))
    }

    /// Context menu of `items` (they must share one parent folder), or of the empty
    /// area of `background` when `items` is empty.
    unsafe fn context_menu(hwnd: HWND, items: &[PathBuf], background: &Path) -> Result<(ComPtr, Menu), String> {
        let mut obj = ptr::null_mut();
        if items.is_empty() {
            let pidl = parse(background)?;
            let mut folder = ptr::null_mut();
            check(SHBindToObject(ptr::null_mut(), pidl.0, ptr::null_mut(), &IID_SHELL_FOLDER, &mut folder))?;
            let folder = ComPtr(folder);
            check((folder.vtable::<FolderVtbl>().create_view_object)(folder.0, hwnd, &IID_MENU, &mut obj))?;
        } else {
            let pidls = items.iter().map(|p| parse(p)).collect::<Result<Vec<_>, _>>()?;
            let (folder, _) = bind_parent(&pidls[0])?;
            let children: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| ILFindLastID(p.0) as *const _).collect();
            check((folder.vtable::<FolderVtbl>().get_ui_object_of)(
                folder.0,
                hwnd,
                children.len() as u32,
                children.as_ptr(),
                &IID_MENU,
                ptr::null_mut(),
                &mut obj,
            ))?;
        }
        if obj.is_null() {
            return Err("Windows Shell: no context menu".into());
        }
        let obj = ComPtr(obj);
        let menu = Menu(CreatePopupMenu());
        if menu.0.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut flags = CMF_NORMAL | CMF_EXPLORE;
        if GetKeyState(VK_SHIFT as i32) < 0 {
            flags |= CMF_EXTENDEDVERBS;
        }
        check((obj.vtable::<MenuVtbl>().query_menu)(obj.0, menu.0, 0, FIRST_ID, LAST_ID, flags))?;
        Ok((obj, menu))
    }

    /// The menu object that draws owner-drawn items and fills submenus ("Send to",
    /// "Open with") on demand, with the interface level it supports.
    struct Handler {
        obj: ComPtr,
        level3: bool,
    }

    unsafe extern "system" fn menu_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _: usize, data: usize) -> LRESULT {
        if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
            let h = &*(data as *const Handler);
            let vt = h.obj.vtable::<MenuVtbl>();
            if h.level3 {
                let mut result = 0;
                if (vt.handle_msg2)(h.obj.0, msg, wp, lp, &mut result) >= 0 {
                    return result;
                }
            } else if msg != WM_MENUCHAR && (vt.handle_msg)(h.obj.0, msg, wp, lp) >= 0 {
                return if msg == WM_INITMENUPOPUP { 0 } else { 1 };
            }
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }

    struct Subclass(HWND);
    impl Drop for Subclass {
        fn drop(&mut self) {
            unsafe { RemoveWindowSubclass(self.0, Some(menu_proc), SUBCLASS) };
        }
    }

    /// Canonical verb of a command ("delete", "rename", "properties" …), if it has one.
    unsafe fn verb(obj: &ComPtr, command: u32) -> String {
        let mut buf = [0u16; 128];
        let hr = (obj.vtable::<MenuVtbl>().get_command_string)(
            obj.0,
            (command - FIRST_ID) as usize,
            GCS_VERBW,
            ptr::null_mut(),
            buf.as_mut_ptr() as *mut u8,
            buf.len() as u32,
        );
        if hr < 0 {
            return String::new();
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    }

    /// What the user picked in the Windows menu.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Picked {
        /// Nothing (the menu was dismissed).
        None,
        /// A command the shell carried out.
        Done,
        /// "Rename": the shell cannot rename in our list, the caller does it.
        Rename,
    }

    /// Show the Explorer context menu at the mouse cursor for `items`, or for the
    /// empty area of `folder` when `items` is empty, and run the chosen command.
    /// Items from different folders get the menu of those in the first one's folder.
    /// Must run on the window's UI thread; the native menu pumps Windows messages.
    pub fn show(hwnd: isize, items: &[PathBuf], folder: &Path) -> Result<Picked, String> {
        if hwnd == 0 {
            return Err("Invalid window".into());
        }
        let parent = items.first().and_then(|p| p.parent()).map(Path::to_path_buf);
        let items: Vec<PathBuf> = match &parent {
            Some(dir) => items.iter().filter(|p| p.parent() == Some(dir.as_path())).cloned().collect(),
            // a drive root: no parent, show it alone
            None => items.iter().take(1).cloned().collect(),
        };
        let directory = if items.is_empty() { folder.to_path_buf() } else { parent.unwrap_or_else(|| items[0].clone()) };
        unsafe {
            let hr = CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
            let _apartment = Apartment(hr >= 0);
            LAST.with(|l| l.borrow_mut().take());
            let hwnd = hwnd as HWND;
            let (obj, menu) = context_menu(hwnd, &items, folder)?;
            let (handler, level3) = match obj.query(&IID_MENU3) {
                Some(m) => (Some(m), true),
                None => (obj.query(&IID_MENU2), false),
            };
            let handler = handler.map(|obj| Handler { obj, level3 });
            let subclass = match &handler {
                Some(h) if SetWindowSubclass(hwnd, Some(menu_proc), SUBCLASS, h as *const Handler as usize) != 0 => Some(Subclass(hwnd)),
                _ => None,
            };
            let mut pos = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pos);
            // the menu must belong to the foreground window or it never closes on an outside click
            SetForegroundWindow(hwnd);
            let command = TrackPopupMenuEx(menu.0, TPM_RETURNCMD | TPM_RIGHTBUTTON, pos.x, pos.y, hwnd, ptr::null()) as u32;
            drop(subclass);
            drop(handler);
            if command < FIRST_ID || command > LAST_ID {
                return Ok(Picked::None);
            }
            if verb(&obj, command).eq_ignore_ascii_case("rename") {
                return Ok(Picked::Rename);
            }
            let mut info: CMINVOKECOMMANDINFOEX = std::mem::zeroed();
            let dir = wide(&directory);
            info.cbSize = std::mem::size_of_val(&info) as u32;
            info.fMask = 0x0000_4000 | CMIC_MASK_PTINVOKE; // CMIC_MASK_UNICODE
            if GetKeyState(VK_SHIFT as i32) < 0 {
                info.fMask |= CMIC_MASK_SHIFT_DOWN;
            }
            info.hwnd = hwnd;
            info.lpVerb = (command - FIRST_ID) as usize as *const u8;
            info.lpVerbW = (command - FIRST_ID) as usize as *const u16;
            info.lpDirectoryW = dir.as_ptr();
            info.nShow = SW_SHOWNORMAL;
            info.ptInvoke = pos;
            let hr = (obj.vtable::<MenuVtbl>().invoke)(obj.0, &info as *const _ as *const CMINVOKECOMMANDINFO);
            LAST.with(|l| *l.borrow_mut() = Some(obj));
            // cancelled by the user (e.g. a confirmation answered "No")
            if hr == 0x800704C7u32 as i32 {
                return Ok(Picked::None);
            }
            check(hr)?;
            Ok(Picked::Done)
        }
    }

    /// Run the shell verb `verb` ("eject", "properties" …) of one item without
    /// showing a menu, as Explorer's own commands do.
    pub fn invoke_verb(hwnd: isize, item: &Path, verb: &str) -> Result<(), String> {
        unsafe {
            let hr = CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
            let _apartment = Apartment(hr >= 0);
            let hwnd = hwnd as HWND;
            let (obj, _menu) = context_menu(hwnd, &[item.to_path_buf()], item)?;
            let ansi: Vec<u8> = verb.bytes().chain(Some(0)).collect();
            let wverb: Vec<u16> = verb.encode_utf16().chain(Some(0)).collect();
            let mut info: CMINVOKECOMMANDINFOEX = std::mem::zeroed();
            info.cbSize = std::mem::size_of_val(&info) as u32;
            info.fMask = 0x0000_4000; // CMIC_MASK_UNICODE
            info.hwnd = hwnd;
            info.lpVerb = ansi.as_ptr();
            info.lpVerbW = wverb.as_ptr();
            info.nShow = SW_SHOWNORMAL;
            let hr = (obj.vtable::<MenuVtbl>().invoke)(obj.0, &info as *const _ as *const CMINVOKECOMMANDINFO);
            LAST.with(|l| *l.borrow_mut() = Some(obj));
            check(hr)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn items_of(items: &[PathBuf], folder: &Path) -> Vec<String> {
            unsafe {
                let hr = CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
                let _apartment = Apartment(hr >= 0);
                let (obj, menu) = context_menu(ptr::null_mut(), items, folder).unwrap();
                let mut verbs = Vec::new();
                for i in 0..GetMenuItemCount(menu.0) {
                    let id = GetMenuItemID(menu.0, i);
                    if (FIRST_ID..=LAST_ID).contains(&id) {
                        verbs.push(verb(&obj, id));
                    }
                }
                verbs
            }
        }

        #[test]
        fn file_menu_has_shell_verbs() {
            let dir = std::env::temp_dir().join(format!("quokka-shell-menu-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let a = dir.join("a.txt");
            let b = dir.join("b.txt");
            std::fs::write(&a, "x").unwrap();
            std::fs::write(&b, "y").unwrap();
            let verbs = items_of(&[a.clone(), b.clone()], &dir);
            assert!(verbs.iter().any(|v| v.eq_ignore_ascii_case("properties")), "{verbs:?}");
            assert!(verbs.iter().any(|v| v.eq_ignore_ascii_case("delete")), "{verbs:?}");
            let background = items_of(&[], &dir);
            assert!(background.iter().any(|v| v.eq_ignore_ascii_case("properties")), "{background:?}");
            let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) + "\\";
            let verbs = items_of(&[PathBuf::from(&drive)], Path::new(&drive));
            assert!(verbs.iter().any(|v| v.eq_ignore_ascii_case("properties")), "{verbs:?}");
            std::fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn missing_item_is_an_error() {
            assert!(show(0, &[], Path::new("C:\\")).is_err());
            unsafe {
                let hr = CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
                let _apartment = Apartment(hr >= 0);
                assert!(context_menu(ptr::null_mut(), &[PathBuf::from(r"C:\not\here\x.txt")], Path::new("C:\\")).is_err());
            }
        }
    }
}

#[cfg(windows)]
pub use imp::{invoke_verb, show, Picked};
