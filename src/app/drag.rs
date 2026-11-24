//! File drag and drop: between the panes, onto folders, out to other apps and in
//! from them. Ctrl — copy, Shift — move, no key — move within one drive and copy
//! between drives (as in Explorer).

use quokka_domain::{tr, trf};
use super::*;
use crate::dnd;
use eframe::egui::{CursorIcon, LayerId, Order, Pos2};
use std::path::Path;

/// Where files can be dropped in one pane: onto a folder row, or anywhere else in
/// the pane — into the folder it shows (`None` for «This PC» and search results).
pub struct DropZone {
    pub area: Rect,
    pub folder: Option<PathBuf>,
    pub rows: Vec<(Rect, PathBuf)>,
}

impl Default for DropZone {
    fn default() -> Self {
        DropZone { area: Rect::NOTHING, folder: None, rows: Vec::new() }
    }
}

/// Files picked up in a pane and being dragged inside the window.
pub struct FileDrag {
    pub sources: Vec<PathBuf>,
    pub from: usize,
}

pub(super) struct Target {
    side: usize,
    dest: PathBuf,
    highlight: Rect,
    ok: bool,
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn what(sources: &[PathBuf]) -> String {
    match sources {
        [one] => format!("«{}»", name_of(one)),
        _ => items_text(sources.len() as u64),
    }
}

impl QuokkaApp {
    pub(super) fn drag_and_drop(&mut self, ctx: &egui::Context) {
        self.internal_drag(ctx);
        self.external_drop(ctx);
    }

    fn drop_target(&self, pos: Pos2, sources: &[PathBuf]) -> Option<Target> {
        for (side, zone) in self.zones.iter().enumerate() {
            let Some(z) = zone else { continue };
            if !z.area.contains(pos) {
                continue;
            }
            // a folder row — except one of the dragged folders, which falls through to the pane
            if let Some((r, p)) = z.rows.iter().find(|(r, p)| r.contains(pos) && !sources.contains(p)) {
                return Some(Target { side, dest: p.clone(), highlight: *r, ok: quokka_infra::archive::location(p).is_none() && dnd::can_drop(sources, p) });
            }
            let dest = z.folder.clone().unwrap_or_default();
            let ok = quokka_infra::archive::location(&dest).is_none() && dnd::can_drop(sources, &dest);
            return Some(Target { side, dest, highlight: z.area, ok });
        }
        None
    }

    fn drop_files(&mut self, sources: Vec<PathBuf>, dest: PathBuf, ctrl: bool, shift: bool) {
        let mv = !sources.iter().any(|s| quokka_infra::archive::is_member(s)) && dnd::is_move(ctrl, shift, &sources, &dest);
        let overwrite = false;
        self.enqueue(if mv { OpSpec::Move { sources, dest, overwrite } } else { OpSpec::Copy { sources, dest, overwrite } });
    }

    /// Files dragged by their name in a pane.
    fn internal_drag(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.file_drag else { return };
        if !self.idle() || ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.file_drag = None;
            return;
        }
        let (down, pos, mods) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos(), i.modifiers));
        if down {
            // left the window (or reached its edge — a maximized window has nowhere else
            // to go): hand the drag over to the OS so it can land in Explorer etc.
            let outside = pos.map_or(true, |p| !ctx.content_rect().shrink(6.0).contains(p));
            if outside && self.hwnd != 0 && cfg!(windows) {
                let d = self.file_drag.take().expect("checked above");
                if d.sources.iter().any(|s| quokka_infra::archive::is_member(s)) {
                    self.toast(tr!("Сначала извлеките файлы из архива через F5", "Extract files with F5 before dragging to another app"), false);
                    return;
                }
                if d.sources.iter().any(|s| quokka_domain::remote::is_remote(s)) {
                    self.toast(tr!("Файлы с сервера сначала скопируйте на диск (F5)", "Copy server files to a local disk first (F5)"), false);
                    return;
                }
                dnd::drag_out(self.hwnd, &d.sources);
                self.release_pointer = true;
                self.reload_all();
                ctx.request_repaint();
                return;
            }
            if let Some(pos) = pos {
                let t = self.drop_target(pos, &d.sources);
                let mv = t.as_ref().map(|t| !d.sources.iter().any(|s| quokka_infra::archive::is_member(s)) && dnd::is_move(mods.command, mods.shift, &d.sources, &t.dest));
                ctx.set_cursor_icon(match (&t, mv) {
                    (Some(t), Some(true)) if t.ok => CursorIcon::Move,
                    (Some(t), Some(false)) if t.ok => CursorIcon::Copy,
                    _ => CursorIcon::NoDrop,
                });
                paint(ctx, pos, &d.sources, t.as_ref(), mv);
            }
            return;
        }
        let d = self.file_drag.take().expect("checked above");
        let Some(t) = pos.and_then(|p| self.drop_target(p, &d.sources)) else { return };
        if t.ok {
            // the source pane's marks are what was dragged: `enqueue` clears them
            self.active = d.from;
            self.drop_files(d.sources, t.dest, mods.command, mods.shift);
        }
    }

    /// Files dragged in from another app (or out of this window and back).
    fn external_drop(&mut self, ctx: &egui::Context) {
        let (hovering, hovered, dropped) = ctx.input(|i| {
            let paths = |v: &[egui::HoveredFile]| v.iter().filter_map(|f| f.path.clone()).collect::<Vec<_>>();
            (
                !i.raw.hovered_files.is_empty(),
                paths(&i.raw.hovered_files),
                i.raw.dropped_files.iter().map(|f| f.path()).filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_path_buf()).collect::<Vec<_>>(),
            )
        });
        if !hovering && dropped.is_empty() {
            return;
        }
        // during an OS drag the window gets no mouse events: ask the OS where the cursor is
        let pos = dnd::cursor_in_client(self.hwnd)
            .map(|(x, y)| pos2(x, y) / ctx.pixels_per_point())
            .or_else(|| ctx.input(|i| i.pointer.latest_pos()));
        let Some(pos) = pos else { return };
        let (ctrl, shift) = dnd::keys();
        if !dropped.is_empty() {
            if !self.idle() {
                self.toast(tr!("Закройте диалог, чтобы перенести файлы", "Close the dialog to transfer files"), true);
                return;
            }
            match self.drop_target(pos, &dropped) {
                Some(t) if t.ok => {
                    self.active = t.side;
                    self.drop_files(dropped, t.dest, ctrl, shift);
                }
                Some(t) if t.dest.as_os_str().is_empty() => self.toast(tr!("Откройте папку, чтобы положить в неё файлы", "Open a folder to drop files into it"), true),
                _ => {}
            }
        } else if self.idle() {
            let t = self.drop_target(pos, &hovered);
            let mv = t.as_ref().map(|t| dnd::is_move(ctrl, shift, &hovered, &t.dest));
            paint(ctx, pos, &hovered, t.as_ref(), mv);
            // keys and cursor are polled: keep the hint up to date
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }
}

/// Highlight the drop target and show what will happen next to the cursor.
fn paint(ctx: &egui::Context, pos: Pos2, sources: &[PathBuf], target: Option<&Target>, mv: Option<bool>) {
    let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("file-dnd")));
    let ok = target.is_some_and(|t| t.ok);
    if let Some(t) = target.filter(|t| t.ok) {
        p.rect(
            t.highlight.shrink(1.0),
            rad(8.0),
            Color32::from_rgba_unmultiplied(22, 119, 255, 22),
            Stroke::new(2.0, BLUE), eframe::egui::StrokeKind::Middle,
        );
    }
    if sources.is_empty() {
        return;
    }
    let title = match (target, mv) {
        (Some(t), Some(mv)) if ok => {
            let verb = if mv { tr!("Переместить", "Move") } else { tr!("Копировать", "Copy") };
            trf!("{verb} {} в «{}»", "{verb} {} to “{}”", what(sources), name_of(&t.dest))
        }
        _ => trf!("{} — сюда нельзя", "{} — cannot drop here", what(sources)),
    };
    let hint = tr!("Ctrl — копировать · Shift — переместить", "Ctrl — copy · Shift — move");
    let g1 = p.layout_no_wrap(title, semibold(14.0), if ok { TEXT() } else { MUTED() });
    let g2 = p.layout_no_wrap(hint.into(), regular(12.5), MUTED());
    let w = g1.size().x.max(g2.size().x) + 24.0;
    let h = g1.size().y + g2.size().y + 20.0;
    let screen = ctx.content_rect();
    let mut at = pos + vec2(18.0, 16.0);
    at.x = at.x.min(screen.right() - w - 4.0);
    at.y = at.y.min(screen.bottom() - h - 4.0);
    let r = Rect::from_min_size(at, vec2(w, h));
    p.rect(r.translate(vec2(0.0, 2.0)), rad(9.0), Color32::from_black_alpha(18), Stroke::NONE, eframe::egui::StrokeKind::Middle);
    p.rect(r, rad(9.0), CARD(), Stroke::new(1.0, if ok { BLUE } else { BORDER() }), eframe::egui::StrokeKind::Middle);
    let y2 = r.top() + 8.0 + g1.size().y + 4.0;
    p.galley(r.min + vec2(12.0, 8.0), g1, TEXT());
    p.galley(pos2(r.left() + 12.0, y2), g2, MUTED());
}
