//! Quick view (Ctrl+Q): the other pane shows the file under the cursor and follows it
//! as the cursor moves. Photos are decoded off the UI thread at screen size, the last
//! few stay in memory and the neighbours of the current photo are decoded ahead, so
//! stepping through a folder of pictures with the arrow keys is instant. Text files
//! show their beginning; anything else gets a card with its icon, size and date.

use quokka_domain::{tr, trf};
use super::preview::{self, Content};
use super::*;
use quokka_domain::entry::Entry;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Photos are scaled down to this many pixels on the longer side: sharp in a pane
/// even on a 4K screen, and much lighter to upload than the full picture.
const MAX_SIDE: u32 = 2048;
/// Decoded files kept in memory (most recently shown last).
const CACHE: usize = 12;
/// While a decode is already running, wait this long after the cursor stops before
/// starting the next one, so holding an arrow key does not decode every photo passed.
const SETTLE: Duration = Duration::from_millis(90);
/// Decoder threads at most, including the ones still finishing a cancelled request.
const MAX_RUNNING: usize = 3;
/// How many rows to look past the cursor for the next / previous photo to decode ahead.
const LOOKAHEAD: usize = 12;
/// Text shown for a text file (the viewer, F3, shows more).
const TEXT_LIMIT: usize = 64 * 1024;

enum Shown {
    Photo(egui::TextureHandle),
    Text(String),
    Error(String),
}

struct Loaded {
    path: PathBuf,
    shown: Shown,
    info: String,
}

struct Job {
    path: PathBuf,
    cancel: Arc<AtomicBool>,
}

type Result = std::result::Result<(Content, String), String>;

pub(super) struct QuickView {
    /// The pane it takes the place of.
    pub side: usize,
    ctx: egui::Context,
    target: Option<PathBuf>,
    target_since: Instant,
    /// The last photo or text drawn: kept on screen, dimmed, while the next one loads.
    last: Option<PathBuf>,
    cache: VecDeque<Loaded>,
    jobs: Vec<Job>,
    running: Arc<AtomicUsize>,
    tx: Sender<(PathBuf, Result)>,
    rx: Receiver<(PathBuf, Result)>,
}

impl Drop for QuickView {
    fn drop(&mut self) {
        for j in &self.jobs {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Whether quick view can draw the contents of this file.
fn previewable(e: &Entry) -> bool {
    !e.is_dir && !e.is_drive && !e.is_parent && (is_photo(e) || preview::is_text_extension(&e.ext))
}

fn is_photo(e: &Entry) -> bool {
    !e.is_dir && preview::is_photo_extension(&e.ext)
}

impl QuickView {
    pub(super) fn new(ctx: &egui::Context, side: usize) -> Self {
        let (tx, rx) = mpsc::channel();
        QuickView {
            side,
            ctx: ctx.clone(),
            target: None,
            target_since: Instant::now(),
            last: None,
            cache: VecDeque::new(),
            jobs: Vec::new(),
            running: Arc::new(AtomicUsize::new(0)),
            tx,
            rx,
        }
    }

    fn cached(&self, path: &Path) -> Option<&Loaded> {
        self.cache.iter().find(|l| l.path == path)
    }

    fn loading(&self, path: &Path) -> bool {
        self.jobs.iter().any(|j| j.path == path)
    }

    /// Decoded results that came in: turn photos into textures and cache them.
    fn receive(&mut self) {
        while let Ok((path, result)) = self.rx.try_recv() {
            let Some(at) = self.jobs.iter().position(|j| j.path == path) else { continue };
            self.jobs.remove(at);
            let (shown, info) = match result {
                Ok((Content::Photo { pixels, dimensions }, info)) => {
                    let texture = self.ctx.load_texture(format!("quick-view:{}", path.display()), pixels, egui::TextureOptions::LINEAR);
                    (Shown::Photo(texture), format!("{} × {} · {info}", dimensions.0, dimensions.1))
                }
                Ok((Content::Text(mut text), info)) => {
                    if text.len() > TEXT_LIMIT {
                        let mut end = TEXT_LIMIT;
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        text.truncate(end);
                    }
                    (Shown::Text(text), info)
                }
                Err(e) => (Shown::Error(e), String::new()),
            };
            self.cache.retain(|l| l.path != path);
            if self.cache.len() >= CACHE {
                self.cache.pop_front();
            }
            self.cache.push_back(Loaded { path, shown, info });
        }
    }

    fn start(&mut self, path: PathBuf) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (flag, tx, ctx, running, file) = (cancel.clone(), self.tx.clone(), self.ctx.clone(), self.running.clone(), path.clone());
        running.fetch_add(1, Ordering::Relaxed);
        let spawned = std::thread::Builder::new().name("quokka-quick-view".into()).spawn(move || {
            let result = preview::load(&file, &flag, MAX_SIDE).map_err(|e| e.to_string());
            running.fetch_sub(1, Ordering::Relaxed);
            if !flag.load(Ordering::Relaxed) && tx.send((file, result)).is_ok() {
                ctx.request_repaint();
            }
        });
        match spawned {
            Ok(_) => self.jobs.push(Job { path, cancel }),
            Err(e) => {
                self.running.fetch_sub(1, Ordering::Relaxed);
                self.cache.push_back(Loaded { path, shown: Shown::Error(e.to_string()), info: String::new() });
            }
        }
    }

    /// The nearest photos before and after the cursor, to decode ahead of time.
    fn neighbours(panel: &Panel) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let c = panel.cursor;
        let after = (c + 1..panel.visible.len()).take(LOOKAHEAD);
        let before = (0..c).rev().take(LOOKAHEAD);
        for range in [after.collect::<Vec<_>>(), before.collect()] {
            if let Some(e) = range.into_iter().filter_map(|i| panel.entry_at(i)).find(|e| is_photo(e)) {
                out.push(e.path.clone());
            }
        }
        out
    }

    /// Decide what to decode now: the file under the cursor first, then its neighbours.
    fn schedule(&mut self, panel: &Panel) {
        let current = panel.current().filter(|e| previewable(e)).map(|e| e.path.clone());
        // a request nobody needs any more is dropped (the thread finishes on its own)
        let mut wanted: Vec<PathBuf> = current.iter().cloned().collect();
        let remote = quokka_domain::remote::is_remote(&panel.path);
        let current_ready = current.as_ref().map_or(true, |p| self.cached(p).is_some());
        if current_ready && !remote {
            wanted.extend(Self::neighbours(panel));
        }
        for j in &self.jobs {
            if !wanted.contains(&j.path) {
                j.cancel.store(true, Ordering::Relaxed);
            }
        }
        self.jobs.retain(|j| wanted.contains(&j.path));
        for path in wanted {
            if self.cached(&path).is_some() || self.loading(&path) {
                continue;
            }
            if self.running.load(Ordering::Relaxed) >= MAX_RUNNING {
                // a decoder will free up and ask for a repaint
                break;
            }
            let is_current = Some(&path) == current.as_ref();
            if is_current && self.running.load(Ordering::Relaxed) > 0 {
                let waited = self.target_since.elapsed();
                if waited < SETTLE {
                    self.ctx.request_repaint_after(SETTLE - waited);
                    break;
                }
            }
            self.start(path);
            if !is_current {
                // one look-ahead at a time keeps the current photo first in line
                break;
            }
        }
    }

    pub(super) fn ui(&mut self, ui: &mut Ui, rect: Rect, panel: &Panel, file_icons: Option<&mut crate::file_icons::FileIcons>) {
        self.receive();
        let entry = panel.current().filter(|e| !e.is_parent).cloned();
        let path = entry.as_ref().map(|e| e.path.clone());
        if path != self.target {
            self.target = path;
            self.target_since = Instant::now();
        }
        self.schedule(panel);
        if let Some(e) = entry.as_ref().filter(|e| previewable(e) && self.cached(&e.path).is_some()) {
            self.last = Some(e.path.clone());
        }

        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0.0, CARD);
        let inner = rect.shrink2(vec2(14.0, 12.0));
        // header: file name and what it is
        let head = Rect::from_min_size(inner.min, vec2(inner.width(), 40.0));
        p.rect_filled(head, 8.0, HEADER_BG);
        let title = entry.as_ref().map_or_else(|| tr!("Быстрый просмотр", "Quick view").to_string(), |e| e.name.clone());
        let ready = entry.as_ref().and_then(|e| self.cached(&e.path));
        let info = match (&entry, ready) {
            (_, Some(l)) if !l.info.is_empty() => l.info.clone(),
            (Some(e), _) if e.is_dir => e.type_name.to_string(),
            (Some(e), _) => format_size(e.size),
            (None, _) => String::new(),
        };
        let name_w = (head.width() - 24.0) * if info.is_empty() { 1.0 } else { 0.62 };
        let shown = elide_right(&p, &title, &semibold(13.0), name_w);
        p.text(pos2(head.left() + 12.0, head.center().y), Align2::LEFT_CENTER, shown, semibold(13.0), TEXT);
        p.text(pos2(head.right() - 12.0, head.center().y), Align2::RIGHT_CENTER, &info, regular(11.5), TEXT_SOFT);
        // footer: how to leave
        let foot_h = 22.0;
        let foot = Rect::from_min_max(pos2(inner.left(), inner.bottom() - foot_h), inner.max);
        let key = super::hotkeys::menu_key(&Action::QuickView, "Ctrl+Q");
        let view = super::hotkeys::menu_key(&Action::View, "F3");
        let hint = trf!("{key} — закрыть быстрый просмотр · {view} — открыть в просмотрщике", "{key} — close quick view · {view} — open in the viewer");
        let hint = elide_right(&p, &hint, &regular(11.5), foot.width());
        p.text(foot.center(), Align2::CENTER_CENTER, hint, regular(11.5), MUTED);
        let body = Rect::from_min_max(pos2(inner.left(), head.bottom() + 10.0), pos2(inner.right(), foot.top() - 8.0));
        if body.height() < 20.0 {
            return;
        }

        let Some(e) = entry else {
            p.text(body.center(), Align2::CENTER_CENTER, tr!("Нет файла под курсором", "No file under the cursor"), regular(12.5), MUTED);
            return;
        };
        if !previewable(&e) {
            card(&p, body, &e, file_icons);
            return;
        }
        // the file itself, or the previous one dimmed while it loads
        let (loaded, stale) = match ready {
            Some(l) => (Some(l), false),
            None => (self.last.as_deref().and_then(|l| self.cached(l)), true),
        };
        match loaded {
            Some(Loaded { shown: Shown::Photo(texture), .. }) => {
                p.rect_filled(body, 8.0, INPUT_BG);
                let native = texture.size_vec2();
                let scale = (body.width() / native.x).min(body.height() / native.y).min(1.0);
                let r = Rect::from_center_size(body.center(), native * scale);
                let tint = if stale { Color32::from_white_alpha(90) } else { Color32::WHITE };
                p.image(texture.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
            }
            Some(Loaded { shown: Shown::Text(text), path, .. }) => {
                p.rect_filled(body, 8.0, INPUT_BG);
                #[allow(deprecated)]
                ui.allocate_ui_at_rect(body.shrink(10.0), |ui| {
                    egui::ScrollArea::both().id_salt(("quick-view-text", path)).auto_shrink([false, false]).show(ui, |ui| {
                        ui.label(RichText::new(text.as_str()).font(mono(12.0)).color(if stale { MUTED } else { TEXT }));
                    });
                });
            }
            Some(Loaded { shown: Shown::Error(err), .. }) if !stale => {
                card(&p, body, &e, file_icons);
                p.text(pos2(body.center().x, body.center().y + 64.0), Align2::CENTER_CENTER, err, regular(12.0), RED);
            }
            _ => {}
        }
        if stale {
            let c = body.center();
            let a = ui.input(|i| i.time) as f32 * 6.0;
            let points = (0..=24).map(|k| {
                let t = a + k as f32 / 24.0 * std::f32::consts::PI * 1.5;
                c + vec2(t.cos(), t.sin()) * 14.0
            }).collect();
            p.add(egui::Shape::line(points, Stroke::new(2.5, BLUE)));
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }
}

/// A file quick view cannot draw: its icon, type, size and date.
fn card(p: &egui::Painter, body: Rect, e: &Entry, file_icons: Option<&mut crate::file_icons::FileIcons>) {
    let c = body.center() - vec2(0.0, 30.0);
    let ir = Rect::from_center_size(c, vec2(64.0, 64.0));
    if e.is_drive {
        icons::drive(p, ir);
    } else if e.is_dir {
        icons::folder(p, ir);
    } else {
        crate::file_icons::paint(file_icons, p, ir, &e.path, e.kind, &e.ext);
    }
    let mut lines = vec![e.type_name.to_string()];
    if !e.is_dir {
        lines.push(format_size(e.size));
    }
    if e.modified.is_some() {
        lines.push(format_time(e.modified));
    }
    p.text(pos2(c.x, ir.bottom() + 18.0), Align2::CENTER_CENTER, lines.join(" · "), regular(12.0), TEXT_SOFT);
}
