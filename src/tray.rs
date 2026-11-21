//! Windows shell integration of the main window: the notification-area (tray) icon,
//! hiding the window to it, and the single-instance mode — a second launch hands its
//! folder to the running window (WM_COPYDATA) and quits.
//!
//! The main window is subclassed (`SetWindowSubclass`) so its own message loop delivers
//! the tray clicks; they don't need egui frames, which a hidden window may not get.
//! What the UI has to act on (quit, open a folder) is queued for the next frame.

use quokka_domain::tr;
use eframe::egui;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// What happened outside egui since the last frame.
#[derive(Default)]
pub struct Events {
    /// «Выход» was picked in the tray menu.
    pub quit: bool,
    /// Folders another launch asked to open.
    pub open: Vec<PathBuf>,
}

static EVENTS: Mutex<Events> = Mutex::new(Events { quit: false, open: Vec::new() });
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// Take the events queued since the last call.
pub fn take_events() -> Events {
    std::mem::take(&mut *EVENTS.lock().unwrap_or_else(|e| e.into_inner()))
}

fn push(f: impl FnOnce(&mut Events)) {
    f(&mut EVENTS.lock().unwrap_or_else(|e| e.into_inner()));
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::Shell::{
        DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
        NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const WM_TRAY: u32 = WM_APP + 0x51;
    const SUBCLASS_ID: usize = 0x514B;
    /// `COPYDATASTRUCT::dwData` of a folder sent by another launch.
    const COPY_OPEN: usize = 0x514B_4B41;
    const CMD_SHOW: usize = 1;
    const CMD_QUIT: usize = 2;
    const TRAY_UID: u32 = 1;

    static HWND_: AtomicIsize = AtomicIsize::new(0);
    static ICON: AtomicIsize = AtomicIsize::new(0);
    static ADDED: AtomicBool = AtomicBool::new(false);
    static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn hwnd() -> HWND {
        HWND_.load(Ordering::Relaxed) as HWND
    }

    /// Hook the main window; call once its handle is known (on the UI thread).
    pub fn install(raw: isize, ctx: &egui::Context) {
        if raw == 0 || HWND_.swap(raw, Ordering::Relaxed) == raw {
            return;
        }
        let _ = CTX.set(ctx.clone());
        TASKBAR_CREATED.get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) });
        unsafe {
            SetWindowSubclass(raw as HWND, Some(subclass_proc), SUBCLASS_ID, 0);
        }
    }

    fn make_icon() -> isize {
        let existing = ICON.load(Ordering::Relaxed);
        if existing != 0 {
            return existing;
        }
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as u32;
        let img = crate::logo::icon_data(size);
        // straight-alpha BGRA, which is what 32-bit icon bitmaps hold
        let bgra: Vec<u8> = img.rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        let mask = vec![0u8; (size.div_ceil(16) * 2 * size) as usize];
        unsafe {
            let color = CreateBitmap(size as i32, size as i32, 1, 32, bgra.as_ptr().cast());
            let mask_bm = CreateBitmap(size as i32, size as i32, 1, 1, mask.as_ptr().cast());
            let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask_bm, hbmColor: color };
            let icon = CreateIconIndirect(&info) as isize;
            DeleteObject(color);
            DeleteObject(mask_bm);
            ICON.store(icon, Ordering::Relaxed);
            icon
        }
    }

    fn notify_data() -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd();
        nid.uID = TRAY_UID;
        nid
    }

    fn add_icon() -> bool {
        let mut nid = notify_data();
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = make_icon() as HICON;
        let tip = wide(crate::title());
        // NOTIFYICONDATAW is packed on x86; fill an aligned local array.
        let mut tooltip = nid.szTip;
        let n = tip.len().min(tooltip.len() - 1);
        tooltip[..n].copy_from_slice(&tip[..n]);
        nid.szTip = tooltip;
        unsafe { Shell_NotifyIconW(NIM_ADD, &nid) != 0 }
    }

    /// Show or remove the tray icon.
    pub fn set_icon(visible: bool) {
        if hwnd().is_null() || ADDED.load(Ordering::Relaxed) == visible {
            return;
        }
        if visible {
            if add_icon() {
                ADDED.store(true, Ordering::Relaxed);
            }
        } else {
            let nid = notify_data();
            unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
            ADDED.store(false, Ordering::Relaxed);
        }
    }

    pub fn icon_shown() -> bool {
        ADDED.load(Ordering::Relaxed)
    }

    /// Hide the window (it stays alive in the tray).
    pub fn hide_window() {
        let h = hwnd();
        if !h.is_null() {
            unsafe { ShowWindow(h, SW_HIDE) };
        }
    }

    /// Bring the window back from the tray (or from minimised) and to the front.
    pub fn show_window() {
        let h = hwnd();
        if h.is_null() {
            return;
        }
        unsafe {
            ShowWindow(h, SW_SHOW);
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            SetForegroundWindow(h);
        }
        if let Some(ctx) = CTX.get() {
            ctx.request_repaint();
        }
    }

    fn menu() {
        let h = hwnd();
        unsafe {
            let m = CreatePopupMenu();
            AppendMenuW(m, MF_STRING, CMD_SHOW, wide(tr!("Открыть Quokka", "Open Quokka")).as_ptr());
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(m, MF_STRING, CMD_QUIT, wide(tr!("Выход", "Exit")).as_ptr());
            SetMenuDefaultItem(m, CMD_SHOW as u32, 0);
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            // without this the menu doesn't close when clicking elsewhere
            SetForegroundWindow(h);
            let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, 0, h, std::ptr::null());
            PostMessageW(h, WM_NULL, 0, 0);
            DestroyMenu(m);
            match cmd as usize {
                CMD_SHOW => show_window(),
                CMD_QUIT => {
                    show_window();
                    push(|e| e.quit = true);
                }
                _ => {}
            }
        }
    }

    unsafe extern "system" fn subclass_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
        match msg {
            WM_TRAY => {
                match lp as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => show_window(),
                    WM_RBUTTONUP | WM_CONTEXTMENU => menu(),
                    _ => {}
                }
                return 0;
            }
            WM_COPYDATA => {
                let cds = lp as *const COPYDATASTRUCT;
                if !cds.is_null() && (*cds).dwData == COPY_OPEN {
                    let n = (*cds).cbData as usize / 2;
                    let path = if n == 0 || (*cds).lpData.is_null() {
                        String::new()
                    } else {
                        String::from_utf16_lossy(std::slice::from_raw_parts((*cds).lpData as *const u16, n))
                    };
                    show_window();
                    if !path.is_empty() {
                        push(|e| e.open.push(PathBuf::from(path)));
                    }
                    return 1;
                }
            }
            WM_NCDESTROY => {
                set_icon(false);
                RemoveWindowSubclass(h, Some(subclass_proc), SUBCLASS_ID);
            }
            m if Some(&m) == TASKBAR_CREATED.get() => {
                // Explorer restarted: the icon is gone, put it back
                if ADDED.load(Ordering::Relaxed) {
                    add_icon();
                }
            }
            _ => {}
        }
        DefSubclassProc(h, msg, wp, lp)
    }

    /// Single-instance mode (`multi == false`): if Quokka already runs, give it `folder`,
    /// bring it to the front and return `true` (this launch should quit).
    pub fn forward_to_running(titles: &[&str], folder: Option<&std::path::Path>, multi: bool) -> bool {
        unsafe {
            // every instance holds the mutex (it lives as long as the process), so the
            // setting works at once, whatever the running instances were started with
            let mutex = CreateMutexW(std::ptr::null(), 0, wide("Local\\Quokka.FileManager.Instance").as_ptr());
            if multi || mutex.is_null() || GetLastError() != ERROR_ALREADY_EXISTS {
                return false;
            }
            let titles: Vec<Vec<u16>> = titles.iter().map(|t| wide(t)).collect();
            for _ in 0..30 {
                let h = titles.iter().map(|t| FindWindowW(std::ptr::null(), t.as_ptr())).find(|h| !h.is_null()).unwrap_or(std::ptr::null_mut());
                if !h.is_null() {
                    let data: Vec<u16> = folder.map(|f| f.as_os_str().to_string_lossy().encode_utf16().collect()).unwrap_or_default();
                    let cds = COPYDATASTRUCT { dwData: COPY_OPEN, cbData: (data.len() * 2) as u32, lpData: data.as_ptr() as *mut _ };
                    AllowSetForegroundWindow(ASFW_ANY);
                    let mut result = 0usize;
                    let ok = SendMessageTimeoutW(h, WM_COPYDATA, 0, &cds as *const _ as LPARAM, SMTO_ABORTIFHUNG, 3000, &mut result);
                    return ok != 0 && result == 1;
                }
                // the running instance may still be creating its window
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn install(_raw: isize, ctx: &egui::Context) {
        let _ = CTX.set(ctx.clone());
    }
    pub fn set_icon(_visible: bool) {}
    pub fn icon_shown() -> bool {
        false
    }
    pub fn hide_window() {}
    pub fn show_window() {}
    pub fn forward_to_running(_titles: &[&str], _folder: Option<&std::path::Path>, _multi: bool) -> bool {
        false
    }
}

pub use imp::{forward_to_running, hide_window, icon_shown, install, set_icon};
