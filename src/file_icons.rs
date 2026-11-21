//! Cached Windows file icons, resolved off the UI thread.

use crate::{icons, iconsrc, util::FileKind};
use eframe::egui::{self, Painter, Rect};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc,
};

const CAPACITY: usize = 2048;
const MAX_PENDING: usize = 128;

pub struct FileIcons {
    entries: HashMap<PathBuf, Option<egui::TextureHandle>>,
    ready: VecDeque<PathBuf>,
    pending: std::collections::HashSet<PathBuf>,
    requests: mpsc::Sender<PathBuf>,
    results: mpsc::Receiver<(PathBuf, Option<iconsrc::Rgba>)>,
}

impl FileIcons {
    pub fn new(ctx: &egui::Context) -> Self {
        let (requests, jobs) = mpsc::channel::<PathBuf>();
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
            for path in jobs {
                let image = iconsrc::shell_icon(&path, 48);
                if done.send((path, image)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
            #[cfg(windows)]
            if initialized {
                unsafe { windows_sys::Win32::System::Com::CoUninitialize() };
            }
        });
        Self {
            entries: HashMap::new(),
            ready: VecDeque::new(),
            pending: Default::default(),
            requests,
            results,
        }
    }

    fn texture(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::TextureId> {
        while let Ok((path, image)) = self.results.try_recv() {
            self.pending.remove(&path);
            let texture = image.map(|image| {
                ctx.load_texture(
                    format!("file-icon:{}", path.display()),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [image.size, image.size],
                        &image.pixels,
                    ),
                    egui::TextureOptions::LINEAR,
                )
            });
            if self.entries.len() >= CAPACITY {
                if let Some(old) = self.ready.pop_front() {
                    self.entries.remove(&old);
                }
            }
            self.ready.push_back(path.clone());
            self.entries.insert(path, texture);
        }
        if let Some(texture) = self.entries.get(path) {
            return texture.as_ref().map(|t| t.id());
        }
        if self.pending.len() < MAX_PENDING && !self.pending.contains(path) {
            if self.requests.send(path.to_path_buf()).is_ok() {
                self.pending.insert(path.to_path_buf());
            }
        }
        None
    }
}

pub fn paint(
    cache: Option<&mut FileIcons>,
    painter: &Painter,
    rect: Rect,
    path: &Path,
    kind: FileKind,
    ext: &str,
) {
    if let Some(texture) = cache.and_then(|cache| cache.texture(painter.ctx(), path)) {
        painter.image(
            texture,
            rect,
            Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        icons::file(painter, rect, kind, ext);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn loads_file_icons_in_background_and_caches_failures() {
        let ctx = egui::Context::default();
        let mut cache = FileIcons::new(&ctx);
        let exe = std::env::current_exe().unwrap();
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let missing = exe.join("missing-file.txt");
        for path in [&exe, &file, &missing] {
            assert!(cache.texture(&ctx, path).is_none());
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !cache.pending.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            cache.texture(&ctx, &exe);
        }
        assert!(cache.pending.is_empty(), "icon worker timed out");
        assert!(cache.texture(&ctx, &exe).is_some());
        assert!(cache.texture(&ctx, &file).is_some());
        assert!(cache.entries.get(&missing).unwrap().is_none());
        assert!(cache.texture(&ctx, &missing).is_none());
        assert!(
            cache.pending.is_empty(),
            "failed icons should not be requested again"
        );
    }
}
