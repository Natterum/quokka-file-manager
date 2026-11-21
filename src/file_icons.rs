//! Cached Windows file icons, resolved off the UI thread.
//!
//! Most files share the icon of their type, so the cache is keyed by extension and a
//! folder of 10 000 `.txt` files costs one shell call and one texture. Only types
//! whose files carry their own icon (programs, shortcuts, icon files …) are looked
//! up per file.

use crate::{icons, iconsrc, util::FileKind};
use eframe::egui::{self, Painter, Rect};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc,
};

const CAPACITY: usize = 2048;
const MAX_PENDING: usize = 128;

/// Extensions whose icon depends on the file itself, not just its type.
const OWN_ICON: [&str; 10] = ["exe", "lnk", "ico", "cur", "ani", "url", "scr", "cpl", "msc", "appref-ms"];

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum IconKey {
    File(PathBuf),
    Type(String),
}

impl IconKey {
    fn of(path: &Path, ext: &str) -> IconKey {
        if OWN_ICON.contains(&ext) {
            IconKey::File(path.to_path_buf())
        } else {
            IconKey::Type(ext.to_string())
        }
    }

    fn load(&self) -> Option<iconsrc::Rgba> {
        match self {
            IconKey::File(path) => iconsrc::shell_icon(path, 48),
            IconKey::Type(ext) => iconsrc::type_icon(ext, 48),
        }
    }
}

pub struct FileIcons {
    entries: HashMap<IconKey, Option<egui::TextureHandle>>,
    ready: VecDeque<IconKey>,
    pending: HashSet<IconKey>,
    requests: mpsc::Sender<IconKey>,
    results: mpsc::Receiver<(IconKey, Option<iconsrc::Rgba>)>,
}

impl FileIcons {
    pub fn new(ctx: &egui::Context) -> Self {
        let (requests, jobs) = mpsc::channel::<IconKey>();
        let (done, results) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            #[cfg(windows)]
            let initialized = unsafe {
                windows_sys::Win32::System::Com::CoInitializeEx(
                    std::ptr::null(),
                    windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32,
                ) >= 0
            };
            for key in jobs {
                let image = key.load();
                if done.send((key, image)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
            #[cfg(windows)]
            if initialized {
                unsafe { windows_sys::Win32::System::Com::CoUninitialize() };
            }
        });
        Self { entries: HashMap::new(), ready: VecDeque::new(), pending: Default::default(), requests, results }
    }

    /// Turn icons the worker has finished into textures.
    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok((key, image)) = self.results.try_recv() {
            self.pending.remove(&key);
            let texture = image.map(|image| {
                ctx.load_texture(
                    format!("file-icon:{key:?}"),
                    egui::ColorImage::from_rgba_unmultiplied([image.size, image.size], &image.pixels),
                    egui::TextureOptions::LINEAR,
                )
            });
            if self.entries.len() >= CAPACITY {
                if let Some(old) = self.ready.pop_front() {
                    self.entries.remove(&old);
                }
            }
            self.ready.push_back(key.clone());
            self.entries.insert(key, texture);
        }
    }

    fn texture(&mut self, ctx: &egui::Context, path: &Path, ext: &str) -> Option<egui::TextureId> {
        self.receive(ctx);
        let key = IconKey::of(path, ext);
        if let Some(texture) = self.entries.get(&key) {
            return texture.as_ref().map(|t| t.id());
        }
        if self.pending.len() < MAX_PENDING && !self.pending.contains(&key) && self.requests.send(key.clone()).is_ok() {
            self.pending.insert(key);
        }
        None
    }
}

pub fn paint(cache: Option<&mut FileIcons>, painter: &Painter, rect: Rect, path: &Path, kind: FileKind, ext: &str) {
    if let Some(texture) = cache.and_then(|cache| cache.texture(painter.ctx(), path, ext)) {
        painter.image(texture, rect, Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    } else {
        icons::file(painter, rect, kind, ext);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn wait(cache: &mut FileIcons, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !cache.pending.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            cache.receive(ctx);
        }
        assert!(cache.pending.is_empty(), "icon worker timed out");
    }

    #[test]
    fn loads_file_icons_in_background_and_caches_failures() {
        let ctx = egui::Context::default();
        let mut cache = FileIcons::new(&ctx);
        let exe = std::env::current_exe().unwrap();
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let missing = exe.join("missing-file.exe");
        assert!(cache.texture(&ctx, &exe, "exe").is_none());
        assert!(cache.texture(&ctx, &file, "toml").is_none());
        assert!(cache.texture(&ctx, &missing, "exe").is_none());
        wait(&mut cache, &ctx);
        assert!(cache.texture(&ctx, &exe, "exe").is_some());
        assert!(cache.texture(&ctx, &file, "toml").is_some());
        assert!(cache.entries.get(&IconKey::File(missing.clone())).unwrap().is_none());
        assert!(cache.texture(&ctx, &missing, "exe").is_none());
        assert!(cache.pending.is_empty(), "failed icons should not be requested again");
    }

    #[test]
    fn files_of_one_type_share_an_icon() {
        let ctx = egui::Context::default();
        let mut cache = FileIcons::new(&ctx);
        for i in 0..500 {
            cache.texture(&ctx, &PathBuf::from(format!(r"C:\nowhere\{i}.txt")), "txt");
        }
        assert_eq!(cache.pending.len(), 1, "one request per type");
        wait(&mut cache, &ctx);
        assert!(cache.texture(&ctx, Path::new(r"C:\elsewhere\x.txt"), "txt").is_some(), "type icons need no real file");
        assert_eq!(cache.entries.len(), 1);
    }
}
