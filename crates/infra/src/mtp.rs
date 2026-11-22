//! Phones, cameras and players connected over MTP (Windows Portable Devices).
//! They have no drive letter: Windows exposes them only through the shell
//! namespace under "This PC". Items are addressed as `mtp://Device/Storage/folder`
//! with the names Explorer shows; this module maps such paths to shell items,
//! reads through `IStream` and writes, deletes and renames through `IFileOperation`.
//!
//! Every call blocks and may take a while (the device answers over USB): callers
//! are background threads, except [`devices`] reading the cached list.

use crate::net::{RemoteWriter, Stat};
use quokka_domain::remote::{self, Protocol, Site};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::RwLock;

/// A connected device: the name Explorer shows and its shell parsing name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub parsing: String,
}

impl Device {
    /// `mtp://Name/`
    pub fn root(&self) -> PathBuf {
        Site { protocol: Protocol::Mtp, user: String::new(), host: self.name.clone(), port: 0 }.path("/")
    }
}

static DEVICES: RwLock<Vec<Device>> = RwLock::new(Vec::new());

/// Devices found by the last [`refresh`].
pub fn devices() -> Vec<Device> {
    DEVICES.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Roots (`mtp://Name/`) of the devices found by the last [`refresh`].
pub fn roots() -> Vec<PathBuf> {
    devices().iter().map(Device::root).collect()
}

/// Look for devices again (enumerates "This PC"; takes tens of milliseconds).
pub fn refresh() {
    #[cfg(windows)]
    {
        let found = imp::enumerate_devices();
        *DEVICES.write().unwrap_or_else(|e| e.into_inner()) = found;
    }
}

/// The site (device) and its root of `path`.
fn device_of(path: &Path) -> io::Result<(Site, String)> {
    remote::split(path)
        .filter(|(s, _)| s.protocol == Protocol::Mtp)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, path.display().to_string()))
}

/// An item on a device (it may not exist yet).
pub struct Remote {
    pub site: Site,
    pub path: String,
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "MTP devices are supported on Windows only"))
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::c_void;

    use std::ptr;
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use windows_sys::core::GUID;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows_sys::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
    use windows_sys::Win32::UI::Shell::{ILFree, ILGetSize, SHCreateItemFromIDList, SHCreateItemFromParsingName, SHGetIDListFromObject};

    const IID_SHELL_ITEM2: GUID = GUID::from_u128(0x7e9fb0d3_919f_4307_ab2e_9b1860310c93);
    const IID_ENUM_ITEMS: GUID = GUID::from_u128(0x70629033_e363_4a28_a567_0db78006e6d7);
    const IID_STREAM: GUID = GUID::from_u128(0x0000000c_0000_0000_c000_000000000046);
    const IID_FILE_OP: GUID = GUID::from_u128(0x947aab5f_0a5c_4c13_b4d6_4bf7836fc9f8);
    const CLSID_FILE_OP: GUID = GUID::from_u128(0x3ad05575_8857_4850_9277_11b85bdb8e09);
    const BHID_ENUM_ITEMS: GUID = GUID::from_u128(0x94f60519_2850_4924_aa5a_d15e84868039);
    const BHID_STREAM: GUID = GUID::from_u128(0x1cebb3ab_7c10_499a_a417_92ca16c4cb83);
    const FMTID_STORAGE: GUID = GUID::from_u128(0xb725f130_47ef_101a_a5f1_02608c9eebac);
    const FMTID_VOLUME: GUID = GUID::from_u128(0x9b174b35_40ff_11d2_a27e_00c04fc30871);
    const PKEY_SIZE: PROPERTYKEY = PROPERTYKEY { fmtid: FMTID_STORAGE, pid: 12 };
    const PKEY_DATE_MODIFIED: PROPERTYKEY = PROPERTYKEY { fmtid: FMTID_STORAGE, pid: 14 };
    const PKEY_FILE_NAME: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x41cf5ae0_f75a_4806_bd87_59c7d9248eb9), pid: 100 };
    const PKEY_FREE_SPACE: PROPERTYKEY = PROPERTYKEY { fmtid: FMTID_VOLUME, pid: 2 };
    const PKEY_CAPACITY: PROPERTYKEY = PROPERTYKEY { fmtid: FMTID_VOLUME, pid: 3 };
    const SIGDN_NORMALDISPLAY: i32 = 0;
    const SIGDN_DESKTOPABSOLUTEPARSING: i32 = 0x8002_8000u32 as i32;
    const SFGAO_FOLDER: u32 = 0x2000_0000;
    const SFGAO_FILESYSTEM: u32 = 0x4000_0000;
    const SFGAO_STREAM: u32 = 0x0040_0000;
    /// FOF_SILENT | FOF_NOCONFIRMATION | FOF_NOCONFIRMMKDIR | FOF_NOERRORUI
    const FOF_QUIET: u32 = 0x0004 | 0x0010 | 0x0200 | 0x0400;
    const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";

    #[repr(C)]
    struct UnknownVtbl {
        query: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    #[repr(C)]
    struct ItemVtbl {
        unknown: UnknownVtbl,
        bind_to_handler: unsafe extern "system" fn(*mut c_void, *mut c_void, *const GUID, *const GUID, *mut *mut c_void) -> i32,
        get_parent: usize,
        get_display_name: unsafe extern "system" fn(*mut c_void, i32, *mut *mut u16) -> i32,
        get_attributes: unsafe extern "system" fn(*mut c_void, u32, *mut u32) -> i32,
        compare: usize,
        // IShellItem2
        get_property_store: usize,
        get_property_store_with_create_object: usize,
        get_property_store_for_keys: usize,
        get_property_description_list: usize,
        update: usize,
        get_property: usize,
        get_clsid: usize,
        get_file_time: unsafe extern "system" fn(*mut c_void, *const PROPERTYKEY, *mut FILETIME) -> i32,
        get_int32: usize,
        get_string: unsafe extern "system" fn(*mut c_void, *const PROPERTYKEY, *mut *mut u16) -> i32,
        get_uint32: usize,
        get_uint64: unsafe extern "system" fn(*mut c_void, *const PROPERTYKEY, *mut u64) -> i32,
    }

    #[repr(C)]
    struct EnumVtbl {
        unknown: UnknownVtbl,
        next: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void, *mut u32) -> i32,
    }

    #[repr(C)]
    struct StreamVtbl {
        unknown: UnknownVtbl,
        read: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *mut u32) -> i32,
    }

    #[repr(C)]
    struct FileOpVtbl {
        unknown: UnknownVtbl,
        advise: usize,
        unadvise: usize,
        set_operation_flags: unsafe extern "system" fn(*mut c_void, u32) -> i32,
        set_progress_message: usize,
        set_progress_dialog: usize,
        set_properties: usize,
        set_owner_window: usize,
        apply_properties_to_item: usize,
        apply_properties_to_items: usize,
        rename_item: unsafe extern "system" fn(*mut c_void, *mut c_void, *const u16, *mut c_void) -> i32,
        rename_items: usize,
        move_item: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u16, *mut c_void) -> i32,
        move_items: usize,
        copy_item: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void, *const u16, *mut c_void) -> i32,
        copy_items: usize,
        delete_item: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> i32,
        delete_items: usize,
        new_item: unsafe extern "system" fn(*mut c_void, *mut c_void, u32, *const u16, *const u16, *mut c_void) -> i32,
        perform_operations: unsafe extern "system" fn(*mut c_void) -> i32,
        get_any_operations_aborted: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32,
    }

    struct Com(*mut c_void);
    // Shell items are created and used on one background thread at a time; the
    // reader may move to another thread, which the shell's free-threaded WPD
    // objects allow.
    unsafe impl Send for Com {}
    impl Com {
        unsafe fn vt<T>(&self) -> &T {
            &**(self.0 as *const *const T)
        }
    }
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { (self.vt::<UnknownVtbl>().release)(self.0) };
        }
    }

    fn init() {
        // background threads: initialize COM once and keep it for the thread's life
        thread_local!(static DONE: () = unsafe {
            CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
        });
        DONE.with(|_| ());
    }

    fn hr(code: i32) -> io::Result<()> {
        if code >= 0 {
            Ok(())
        } else {
            Err(io::Error::other(format!("Windows Shell: HRESULT 0x{:08X}", code as u32)))
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    unsafe fn take_string(p: *mut u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        CoTaskMemFree(p as *const c_void);
        s
    }

    struct Item(Com);

    impl Item {
        fn from_parsing(name: &str) -> io::Result<Item> {
            init();
            let w = wide(name);
            let mut out = ptr::null_mut();
            hr(unsafe { SHCreateItemFromParsingName(w.as_ptr(), ptr::null_mut(), &IID_SHELL_ITEM2, &mut out) })?;
            Ok(Item(Com(out)))
        }

        fn from_idlist(pidl: &[u8]) -> io::Result<Item> {
            init();
            let mut out = ptr::null_mut();
            hr(unsafe { SHCreateItemFromIDList(pidl.as_ptr() as *const ITEMIDLIST, &IID_SHELL_ITEM2, &mut out) })?;
            Ok(Item(Com(out)))
        }

        fn vt(&self) -> &ItemVtbl {
            unsafe { self.0.vt::<ItemVtbl>() }
        }

        fn display(&self, kind: i32) -> String {
            let mut p = ptr::null_mut();
            unsafe {
                if (self.vt().get_display_name)(self.0 .0, kind, &mut p) < 0 {
                    return String::new();
                }
                take_string(p)
            }
        }

        /// The file name with its extension (the display name may hide it).
        fn name(&self) -> String {
            let mut p = ptr::null_mut();
            let n = unsafe {
                if (self.vt().get_string)(self.0 .0, &PKEY_FILE_NAME, &mut p) >= 0 { take_string(p) } else { String::new() }
            };
            if n.is_empty() { self.display(SIGDN_NORMALDISPLAY) } else { n }
        }

        fn attributes(&self, mask: u32) -> u32 {
            let mut out = 0;
            unsafe { (self.vt().get_attributes)(self.0 .0, mask, &mut out) };
            out & mask
        }

        fn is_dir(&self) -> bool {
            let a = self.attributes(SFGAO_FOLDER | SFGAO_STREAM);
            a & SFGAO_FOLDER != 0 && a & SFGAO_STREAM == 0
        }

        fn u64(&self, key: &PROPERTYKEY) -> Option<u64> {
            let mut v = 0;
            (unsafe { (self.vt().get_uint64)(self.0 .0, key, &mut v) } >= 0).then_some(v)
        }

        fn modified(&self) -> Option<SystemTime> {
            let mut ft = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            if unsafe { (self.vt().get_file_time)(self.0 .0, &PKEY_DATE_MODIFIED, &mut ft) } < 0 {
                return None;
            }
            let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
            // 100 ns ticks since 1601-01-01
            let unix = ticks.checked_sub(116_444_736_000_000_000)?;
            Some(UNIX_EPOCH + Duration::from_nanos(unix * 100))
        }

        fn stat(&self) -> Stat {
            let is_dir = self.is_dir();
            Stat { is_dir, is_link: false, len: if is_dir { 0 } else { self.u64(&PKEY_SIZE).unwrap_or(0) }, modified: self.modified(), accessed: None, mode: None }
        }

        fn idlist(&self) -> io::Result<Vec<u8>> {
            let mut pidl = ptr::null_mut();
            hr(unsafe { SHGetIDListFromObject(self.0 .0, &mut pidl) })?;
            let bytes = unsafe { std::slice::from_raw_parts(pidl as *const u8, ILGetSize(pidl) as usize).to_vec() };
            unsafe { ILFree(pidl) };
            Ok(bytes)
        }

        fn children(&self, cancel: &AtomicBool) -> io::Result<Vec<Item>> {
            let mut en = ptr::null_mut();
            hr(unsafe { (self.vt().bind_to_handler)(self.0 .0, ptr::null_mut(), &BHID_ENUM_ITEMS, &IID_ENUM_ITEMS, &mut en) })?;
            let en = Com(en);
            let mut out = Vec::new();
            loop {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                let mut item = ptr::null_mut();
                let mut got = 0;
                let code = unsafe { (en.vt::<EnumVtbl>().next)(en.0, 1, &mut item, &mut got) };
                if code != 0 || got == 0 || item.is_null() {
                    hr(code)?;
                    break;
                }
                // the enumerator hands out IShellItem; ask for IShellItem2
                let item = Com(item);
                let mut item2 = ptr::null_mut();
                let q: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32 =
                    unsafe { std::mem::transmute(item.vt::<UnknownVtbl>().query) };
                if unsafe { q(item.0, &IID_SHELL_ITEM2, &mut item2) } >= 0 && !item2.is_null() {
                    out.push(Item(Com(item2)));
                }
            }
            Ok(out)
        }
    }

    pub fn enumerate_devices() -> Vec<Device> {
        let Ok(pc) = Item::from_parsing(THIS_PC) else { return Vec::new() };
        let Ok(items) = pc.children(&AtomicBool::new(false)) else { return Vec::new() };
        items
            .into_iter()
            .filter(|i| i.attributes(SFGAO_FILESYSTEM | SFGAO_FOLDER) == SFGAO_FOLDER)
            .map(|i| Device { name: i.display(SIGDN_NORMALDISPLAY), parsing: i.display(SIGDN_DESKTOPABSOLUTEPARSING) })
            // portable devices are addressed by their device interface path
            .filter(|d| d.parsing.contains(r"\\?\") && !d.name.is_empty() && !d.name.contains(['/', '\\']))
            .collect()
    }

    /// Path text → ID list of the item, filled while listing folders, so a path
    /// is resolved without walking the device from its root every time.
    static CACHE: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);
    const CACHE_LIMIT: usize = 50_000;

    fn key(site: &Site, path: &str) -> String {
        format!("{}{}", site.host.to_lowercase(), path.to_lowercase())
    }

    fn remember(site: &Site, path: &str, item: &Item) {
        if let Ok(id) = item.idlist() {
            let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            let map = c.get_or_insert_with(HashMap::new);
            if map.len() > CACHE_LIMIT {
                map.clear();
            }
            map.insert(key(site, path), id);
        }
    }

    fn forget(site: &Site, path: &str) {
        let prefix = key(site, path);
        if let Some(map) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            map.retain(|k, _| k != &prefix && !k.starts_with(&format!("{prefix}/")));
        }
    }

    fn cached(site: &Site, path: &str) -> Option<Item> {
        let id = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&key(site, path))?.clone();
        Item::from_idlist(&id).ok()
    }

    fn child_path(dir: &str, name: &str) -> String {
        crate::net::child_path(dir, name)
    }

    fn split_last(path: &str) -> (&str, &str) {
        match path.rfind('/') {
            Some(0) => ("/", &path[1..]),
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("/", path),
        }
    }

    /// The shell item at `path`; `Ok(None)` when nothing is there.
    fn find(site: &Site, path: &str) -> io::Result<Option<Item>> {
        if path == "/" {
            let dev = devices().into_iter().find(|d| d.name.eq_ignore_ascii_case(&site.host));
            let dev = match dev {
                Some(d) => d,
                None => {
                    refresh();
                    match devices().into_iter().find(|d| d.name.eq_ignore_ascii_case(&site.host)) {
                        Some(d) => d,
                        None => return Err(super::not_connected(site)),
                    }
                }
            };
            return Item::from_parsing(&dev.parsing).map(Some);
        }
        if let Some(item) = cached(site, path) {
            return Ok(Some(item));
        }
        let (parent, name) = split_last(path);
        let Some(dir) = find(site, parent)? else { return Ok(None) };
        let mut found = None;
        for child in dir.children(&AtomicBool::new(false))? {
            let n = child.name();
            remember(site, &child_path(parent, &n), &child);
            if found.is_none() && n.eq_ignore_ascii_case(name) {
                found = Some(child);
            }
        }
        Ok(found)
    }

    fn get(site: &Site, path: &str) -> io::Result<Item> {
        find(site, path)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{}{path}", site.url())))
    }

    pub fn list(site: &Site, path: &str, cancel: &AtomicBool) -> io::Result<Vec<(String, Stat)>> {
        let dir = get(site, path)?;
        let mut out = Vec::new();
        for child in dir.children(cancel)? {
            let n = child.name();
            remember(site, &child_path(path, &n), &child);
            out.push((n, child.stat()));
        }
        Ok(out)
    }

    pub fn stat(site: &Site, path: &str) -> io::Result<Option<Stat>> {
        Ok(find(site, path)?.map(|i| i.stat()))
    }

    /// (total, free) of the storage holding `path` (the device itself: its first storage).
    pub fn space(site: &Site, path: &str) -> Option<(u64, u64)> {
        let storage: String = path.split('/').filter(|s| !s.is_empty()).next().map(|s| format!("/{s}")).unwrap_or_default();
        let item = if storage.is_empty() {
            get(site, "/").ok()?.children(&AtomicBool::new(false)).ok()?.into_iter().find(|i| i.u64(&PKEY_CAPACITY).is_some())?
        } else {
            get(site, &storage).ok()?
        };
        Some((item.u64(&PKEY_CAPACITY)?, item.u64(&PKEY_FREE_SPACE)?))
    }

    pub fn shell_path(site: &Site, path: &str) -> Option<String> {
        Some(get(site, path).ok()?.display(SIGDN_DESKTOPABSOLUTEPARSING))
    }

    struct FileOp(Com);

    impl FileOp {
        fn new() -> io::Result<FileOp> {
            init();
            let mut op = ptr::null_mut();
            hr(unsafe { CoCreateInstance(&CLSID_FILE_OP, ptr::null_mut(), CLSCTX_ALL, &IID_FILE_OP, &mut op) })?;
            let op = FileOp(Com(op));
            hr(unsafe { (op.vt().set_operation_flags)(op.0 .0, FOF_QUIET) })?;
            Ok(op)
        }

        fn vt(&self) -> &FileOpVtbl {
            unsafe { self.0.vt::<FileOpVtbl>() }
        }

        fn run(self) -> io::Result<()> {
            hr(unsafe { (self.vt().perform_operations)(self.0 .0) })?;
            let mut aborted = 0;
            unsafe { (self.vt().get_any_operations_aborted)(self.0 .0, &mut aborted) };
            if aborted != 0 {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            Ok(())
        }
    }

    pub fn mkdir(site: &Site, path: &str) -> io::Result<()> {
        let (parent, name) = split_last(path);
        let dir = get(site, parent)?;
        let op = FileOp::new()?;
        let n = wide(name);
        hr(unsafe { (op.vt().new_item)(op.0 .0, dir.0 .0, 0x10, n.as_ptr(), ptr::null(), ptr::null_mut()) })?;
        op.run()?;
        forget(site, path);
        Ok(())
    }

    pub fn delete(site: &Site, path: &str) -> io::Result<()> {
        let item = get(site, path)?;
        let op = FileOp::new()?;
        hr(unsafe { (op.vt().delete_item)(op.0 .0, item.0 .0, ptr::null_mut()) })?;
        op.run()?;
        forget(site, path);
        Ok(())
    }

    pub fn rename(site: &Site, from: &str, to: &str) -> io::Result<()> {
        let item = get(site, from)?;
        let (from_dir, _) = split_last(from);
        let (to_dir, name) = split_last(to);
        let n = wide(name);
        let op = FileOp::new()?;
        if from_dir.eq_ignore_ascii_case(to_dir) {
            hr(unsafe { (op.vt().rename_item)(op.0 .0, item.0 .0, n.as_ptr(), ptr::null_mut()) })?;
        } else {
            let dest = get(site, to_dir)?;
            hr(unsafe { (op.vt().move_item)(op.0 .0, item.0 .0, dest.0 .0, n.as_ptr(), ptr::null_mut()) })?;
        }
        op.run()?;
        forget(site, from);
        forget(site, to);
        Ok(())
    }

    /// Copy the local file `src` into the device folder `dir` as `name`
    /// (replacing an existing item).
    pub fn upload(site: &Site, dir: &str, name: &str, src: &Path) -> io::Result<()> {
        let dest = get(site, dir)?;
        let source = Item::from_parsing(&src.to_string_lossy())?;
        let n = wide(name);
        let op = FileOp::new()?;
        hr(unsafe { (op.vt().copy_item)(op.0 .0, source.0 .0, dest.0 .0, n.as_ptr(), ptr::null_mut()) })?;
        op.run()?;
        forget(site, &child_path(dir, name));
        Ok(())
    }

    pub struct Reader(Com);

    impl Read for Reader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut got = 0u32;
            let len = buf.len().min(u32::MAX as usize) as u32;
            let code = unsafe { (self.0.vt::<StreamVtbl>().read)(self.0 .0, buf.as_mut_ptr() as *mut c_void, len, &mut got) };
            // S_FALSE: fewer bytes than asked — the end of the stream
            if code < 0 {
                hr(code)?;
            }
            Ok(got as usize)
        }
    }

    pub fn reader(site: &Site, path: &str) -> io::Result<Reader> {
        let item = get(site, path)?;
        let mut s = ptr::null_mut();
        hr(unsafe { (item.vt().bind_to_handler)(item.0 .0, ptr::null_mut(), &BHID_STREAM, &IID_STREAM, &mut s) })?;
        Ok(Reader(Com(s)))
    }

}

#[cfg(windows)]
pub(crate) fn not_connected(site: &Site) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotConnected,
        quokka_domain::trf!("Устройство «{}» не подключено", "Device “{}” is not connected", site.host),
    )
}

/// Whether the device of `site` is connected now.
pub fn is_connected(site: &Site) -> bool {
    devices().iter().any(|d| d.name.eq_ignore_ascii_case(&site.host))
}

/// The shell parsing name of the item at `path` (for the Windows menu and
/// property sheet).
pub fn shell_path(path: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let (site, p) = device_of(path).ok()?;
        imp::shell_path(&site, &p).map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

/// (total, free) bytes of the storage holding `path`.
pub fn space(path: &Path) -> Option<(u64, u64)> {
    #[cfg(windows)]
    {
        let (site, p) = device_of(path).ok()?;
        imp::space(&site, &p)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

/// A file being written to a device: it goes to a temporary local file first,
/// [`RemoteWriter::finish`] copies it over (the shell has no streaming write).
struct Upload {
    file: std::fs::File,
    temp: PathBuf,
    site: Site,
    dir: String,
    name: String,
}

impl Write for Upload {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl RemoteWriter for Upload {
    fn finish(self: Box<Self>) -> io::Result<()> {
        let Upload { file, temp, site, dir, name } = *self;
        drop(file);
        #[cfg(windows)]
        let r = imp::upload(&site, &dir, &name, &temp);
        #[cfg(not(windows))]
        let r = { let _ = (&site, &dir, &name); imp::unsupported() };
        let _ = std::fs::remove_file(&temp);
        if let Some(d) = temp.parent() {
            let _ = std::fs::remove_dir(d);
        }
        r
    }
}

impl Remote {
    pub fn open(site: Site, path: String) -> io::Result<Remote> {
        if !is_connected(&site) {
            refresh();
            if !is_connected(&site) {
                #[cfg(windows)]
                return Err(not_connected(&site));
            }
        }
        Ok(Remote { site, path })
    }

    pub fn child(&self, name: &str) -> Remote {
        Remote { site: self.site.clone(), path: crate::net::child_path(&self.path, name) }
    }

    pub fn stat(&self) -> io::Result<Option<Stat>> {
        #[cfg(windows)]
        {
            imp::stat(&self.site, &self.path)
        }
        #[cfg(not(windows))]
        imp::unsupported()
    }

    pub fn lstat(&self) -> io::Result<Option<Stat>> {
        self.stat()
    }

    pub fn list(&self, cancel: &AtomicBool) -> io::Result<Vec<(String, Stat)>> {
        #[cfg(windows)]
        {
            imp::list(&self.site, &self.path, cancel)
        }
        #[cfg(not(windows))]
        {
            let _ = cancel;
            imp::unsupported()
        }
    }

    pub fn mkdir(&self) -> io::Result<()> {
        #[cfg(windows)]
        {
            imp::mkdir(&self.site, &self.path)
        }
        #[cfg(not(windows))]
        imp::unsupported()
    }

    pub fn remove_file(&self) -> io::Result<()> {
        #[cfg(windows)]
        {
            imp::delete(&self.site, &self.path)
        }
        #[cfg(not(windows))]
        imp::unsupported()
    }

    pub fn remove_dir(&self) -> io::Result<()> {
        self.remove_file()
    }

    pub fn rename(&self, to: &Remote) -> io::Result<()> {
        #[cfg(windows)]
        {
            imp::rename(&self.site, &self.path, &to.path)
        }
        #[cfg(not(windows))]
        {
            let _ = to;
            imp::unsupported()
        }
    }

    pub fn create_new(&self) -> io::Result<()> {
        if self.stat()?.is_some() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        self.writer()?.finish()
    }

    pub fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        #[cfg(windows)]
        {
            Ok(Box::new(imp::reader(&self.site, &self.path)?))
        }
        #[cfg(not(windows))]
        imp::unsupported()
    }

    pub fn writer(&self) -> io::Result<Box<dyn RemoteWriter>> {
        let (dir, name) = match self.path.rfind('/') {
            Some(0) => ("/".to_string(), self.path[1..].to_string()),
            Some(i) => (self.path[..i].to_string(), self.path[i + 1..].to_string()),
            None => return Err(io::ErrorKind::InvalidInput.into()),
        };
        if dir == "/" {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, quokka_domain::tr!("Файлы нельзя класть в корень устройства — откройте его хранилище", "Files cannot be put in the device root — open one of its storages")));
        }
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let tmpdir = std::env::temp_dir().join(format!("quokka-mtp-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&tmpdir)?;
        // the temporary file already has the final name: the shell copies it as is
        let temp = tmpdir.join(&name);
        let file = std::fs::File::create(&temp)?;
        Ok(Box::new(Upload { file, temp, site: self.site.clone(), dir, name }))
    }

    pub fn space(&self) -> Option<(u64, u64)> {
        #[cfg(windows)]
        {
            imp::space(&self.site, &self.path)
        }
        #[cfg(not(windows))]
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Lists a connected phone, if there is one (needs a device: run manually).
    #[test]
    #[ignore]
    fn lists_connected_device() {
        refresh();
        let devs = devices();
        eprintln!("devices: {devs:?}");
        let Some(d) = devs.first() else { return };
        let root = Remote::open(remote::split(&d.root()).unwrap().0, "/".into()).unwrap();
        let storages = root.list(&AtomicBool::new(false)).unwrap();
        eprintln!("storages: {storages:?}");
        eprintln!("space: {:?}", space(&d.root()));
        if let Some((name, _)) = storages.first() {
            let s = root.child(name);
            let items = s.list(&AtomicBool::new(false)).unwrap();
            eprintln!("{} items in {name}: {:?}", items.len(), items.iter().take(10).collect::<Vec<_>>());
            let again = s.stat().unwrap().unwrap();
            assert!(again.is_dir);
            // read the first file found a couple of levels down
            let mut stack = vec![s];
            'outer: while let Some(dir) = stack.pop() {
                for (n, st) in dir.list(&AtomicBool::new(false)).unwrap_or_default() {
                    if st.is_dir {
                        if stack.len() < 20 {
                            stack.push(dir.child(&n));
                        }
                    } else if st.len > 0 {
                        let f = dir.child(&n);
                        let mut buf = Vec::new();
                        f.reader().unwrap().take(4096).read_to_end(&mut buf).unwrap();
                        eprintln!("read {} bytes of {} ({} bytes)", buf.len(), f.path, st.len);
                        assert_eq!(buf.len() as u64, st.len.min(4096));
                        assert_eq!(f.stat().unwrap().unwrap().len, st.len);
                        break 'outer;
                    }
                }
            }
        }
    }

    #[test]
    fn enumerating_this_pc_does_not_fail() {
        refresh();
        for d in devices() {
            assert!(d.root().to_string_lossy().starts_with("mtp://"));
        }
    }
}
