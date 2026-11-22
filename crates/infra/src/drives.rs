//! Drives: what kind each one is, its volume label and space, and a background
//! watch that notices drives being plugged in (USB sticks, card readers, phones
//! with mass storage, mounted images, mapped network drives) or removed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DriveKind {
    #[default]
    Fixed,
    Removable,
    Network,
    Optical,
    Ram,
    /// A phone, camera or player over MTP (`mtp://…`).
    Device,
    Unknown,
}

/// What the sidebar shows about a drive.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DriveInfo {
    pub kind: DriveKind,
    /// Volume label ("Windows", "KINGSTON" …); empty when it has none or the drive
    /// is not ready (an empty card reader).
    pub label: String,
    /// (total, free) bytes; `None` when the drive is not ready.
    pub space: Option<(u64, u64)>,
}

impl DriveInfo {
    pub fn used(&self) -> Option<u64> {
        self.space.map(|(t, f)| t.saturating_sub(f))
    }

    /// Share of the drive in use, 0..=1.
    pub fn used_fraction(&self) -> Option<f32> {
        match self.space {
            Some((t, f)) if t > 0 => Some((t.saturating_sub(f)) as f32 / t as f32),
            _ => None,
        }
    }
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Kind of the drive at `root` ("E:\"). Cheap: does not touch the media.
pub fn kind(root: &Path) -> DriveKind {
    if quokka_domain::remote::is_device(root) {
        return DriveKind::Device;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
        let w = wide(root);
        match unsafe { GetDriveTypeW(w.as_ptr()) } {
            2 => DriveKind::Removable, // DRIVE_REMOVABLE
            3 => DriveKind::Fixed,     // DRIVE_FIXED
            4 => DriveKind::Network,   // DRIVE_REMOTE
            5 => DriveKind::Optical,   // DRIVE_CDROM
            6 => DriveKind::Ram,       // DRIVE_RAMDISK
            _ => DriveKind::Unknown,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        DriveKind::Fixed
    }
}

/// Volume label of the drive at `root`. Can block on a dead network drive:
/// call it from a background thread.
pub fn label(root: &Path) -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
        let w = wide(root);
        let mut name = [0u16; 261];
        let ok = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                name.as_mut_ptr(),
                name.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        if ok == 0 {
            return String::new();
        }
        let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        String::from_utf16_lossy(&name[..n])
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        String::new()
    }
}

/// Everything about one drive. Blocking (see [`label`]).
pub fn info(root: &Path) -> DriveInfo {
    if quokka_domain::remote::is_device(root) {
        return DriveInfo { kind: DriveKind::Device, label: quokka_domain::paths::root_label(root), space: crate::mtp::space(root) };
    }
    let space = match (fs2::total_space(root), fs2::available_space(root)) {
        (Ok(t), Ok(f)) => Some((t, f)),
        _ => None,
    };
    DriveInfo { kind: kind(root), label: label(root), space }
}

/// Drive letters present now, as a bitmask (bit 0 = A:).
pub fn mask() -> u32 {
    #[cfg(windows)]
    {
        unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() }
    }
    #[cfg(not(windows))]
    {
        1 << 2
    }
}

/// Roots of the drives in `mask`, from C: on (A: and B: are legacy floppies that
/// make Windows spin up an empty drive).
pub fn roots(mask: u32) -> Vec<PathBuf> {
    (2..26u8).filter(|i| mask & (1 << i) != 0).map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char))).collect()
}

/// A change in the set of drives.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DriveChange {
    pub added: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    /// The first look at devices after start: not something the user just did.
    pub initial: bool,
}

/// Difference between two drive masks.
pub fn diff(old: u32, new: u32) -> DriveChange {
    DriveChange { added: roots(new & !old), removed: roots(old & !new), initial: false }
}

fn diff_sets(old: &[PathBuf], new: &[PathBuf]) -> DriveChange {
    DriveChange {
        added: new.iter().filter(|d| !old.contains(d)).cloned().collect(),
        removed: old.iter().filter(|d| !new.contains(d)).cloned().collect(),
        initial: false,
    }
}

/// Changes whenever a portable device (phone, camera, player) is plugged in or
/// removed: the list of Windows Portable Device interfaces. One cheap PnP call.
fn portable_signature() -> Vec<u16> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
            CM_Get_Device_Interface_List_SizeW, CM_Get_Device_Interface_ListW, CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CR_SUCCESS,
        };
        const GUID_DEVINTERFACE_WPD: windows_sys::core::GUID = windows_sys::core::GUID::from_u128(0x6ac27878_a6fa_4155_ba85_f98f491d4f33);
        unsafe {
            let mut len = 0u32;
            if CM_Get_Device_Interface_List_SizeW(&mut len, &GUID_DEVINTERFACE_WPD, std::ptr::null(), CM_GET_DEVICE_INTERFACE_LIST_PRESENT) != CR_SUCCESS {
                return Vec::new();
            }
            let mut buf = vec![0u16; len as usize + 1];
            if CM_Get_Device_Interface_ListW(&GUID_DEVINTERFACE_WPD, std::ptr::null(), buf.as_mut_ptr(), buf.len() as u32, CM_GET_DEVICE_INTERFACE_LIST_PRESENT) != CR_SUCCESS {
                return Vec::new();
            }
            buf
        }
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Watches the set of drives on a background thread; dropping it stops the watch.
pub struct DriveWatch {
    rx: mpsc::Receiver<DriveChange>,
    stop: Arc<AtomicBool>,
}

impl DriveWatch {
    /// Poll the drive letters and the portable devices every `period` (cheap system
    /// calls that touch no media); "This PC" is enumerated again only when the set
    /// of portable devices changed. Calls `wake` on every change.
    pub fn start(period: Duration, wake: impl Fn() + Send + 'static) -> DriveWatch {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("drive-watch".into())
            .spawn(move || {
                let all = || {
                    let mut v = roots(mask());
                    v.extend(crate::mtp::roots());
                    v
                };
                let mut sig = portable_signature();
                crate::mtp::refresh();
                let mut last = all();
                // devices seen at start: the UI learns about them without a notification
                let first = DriveChange { added: crate::mtp::roots(), removed: Vec::new(), initial: true };
                if !first.added.is_empty() && tx.send(first).is_ok() {
                    wake();
                }
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(period);
                    let now_sig = portable_signature();
                    if now_sig != sig {
                        sig = now_sig;
                        // a phone needs a moment after the PnP event before the shell lists it
                        for _ in 0..6 {
                            let before = crate::mtp::roots();
                            crate::mtp::refresh();
                            if crate::mtp::roots() != before {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    }
                    let now = all();
                    if now != last {
                        let change = diff_sets(&last, &now);
                        last = now;
                        if !change.added.is_empty() || !change.removed.is_empty() {
                            if tx.send(change).is_err() {
                                break;
                            }
                            wake();
                        }
                    }
                }
            })
            .ok();
        DriveWatch { rx, stop }
    }

    /// Changes since the last call, merged into one.
    pub fn poll(&self) -> Option<DriveChange> {
        let mut out: Option<DriveChange> = None;
        while let Ok(c) = self.rx.try_recv() {
            let o = out.get_or_insert_with(|| DriveChange { initial: true, ..Default::default() });
            o.initial &= c.initial;
            for d in c.removed {
                if let Some(i) = o.added.iter().position(|a| *a == d) {
                    o.added.remove(i);
                } else {
                    o.removed.push(d);
                }
            }
            for d in c.added {
                if let Some(i) = o.removed.iter().position(|a| *a == d) {
                    o.removed.remove(i);
                } else {
                    o.added.push(d);
                }
            }
        }
        out.filter(|c| !c.added.is_empty() || !c.removed.is_empty())
    }
}

impl Drop for DriveWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_added_and_removed_letters() {
        let c = 1 << 2;
        let e = 1 << 4;
        let f = 1 << 5;
        let d = diff(c | e, c | f);
        assert_eq!(d.added, vec![PathBuf::from("F:\\")]);
        assert_eq!(d.removed, vec![PathBuf::from("E:\\")]);
        // A: and B: are never listed
        assert!(roots(0b11).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn system_drive_is_fixed_with_space() {
        let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) + "\\";
        let i = info(Path::new(&sys));
        assert_eq!(i.kind, DriveKind::Fixed);
        let (t, f) = i.space.unwrap();
        assert!(t >= f && t > 0);
        assert!(i.used_fraction().unwrap() > 0.0);
        assert!(mask() & (1 << 2) != 0 || !sys.starts_with('C'));
    }
}
