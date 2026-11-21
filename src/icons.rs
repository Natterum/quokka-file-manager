//! Hand-painted vector icons: folders, coloured file tiles and outline glyphs.

use crate::theme::*;
use crate::util::{mix, FileKind, KindColor};
use eframe::egui::{pos2, vec2, Align2, Color32, Painter, Rect, Rounding, Shape, Stroke};
use std::f32::consts::PI;

/// Flat yellow folder.
pub fn folder(p: &Painter, r: Rect) {
    let w = r.width();
    let h = r.height() * 0.86;
    let r = Rect::from_center_size(r.center(), vec2(w, h));
    let rad = (w * 0.1).max(1.5);
    let tab = Rect::from_min_max(pos2(r.left(), r.top()), pos2(r.left() + w * 0.44, r.top() + h * 0.3));
    p.rect_filled(tab, Rounding::same(rad), FOLDER_BACK);
    let back = Rect::from_min_max(pos2(r.left(), r.top() + h * 0.12), r.right_bottom());
    p.rect_filled(back, Rounding::same(rad), FOLDER_BACK);
    let front = Rect::from_min_max(pos2(r.left(), r.top() + h * 0.3), r.right_bottom());
    p.rect_filled(front, Rounding::same(rad), FOLDER_FRONT);
    p.line_segment(
        [pos2(front.left() + rad, front.top() + 0.8), pos2(front.right() - rad, front.top() + 0.8)],
        Stroke::new((h * 0.06).max(1.0), FOLDER_LIGHT),
    );
}

/// Rounded coloured tile with a white type glyph (like `readme.md`, `.pdf`, `.png` in the reference).
pub fn file(p: &Painter, r: Rect, kind: FileKind, ext: &str) {
    let c = kind.color();
    let side = r.height().min(r.width());
    let t = Rect::from_center_size(r.center(), vec2(side, side));
    p.rect_filled(t, Rounding::same(side * 0.24), c);
    let inner = t.shrink(side * 0.2);
    let white = Color32::WHITE;
    let s = Stroke::new((side * 0.075).max(1.2), white);
    let at = |x: f32, y: f32| pos2(inner.left() + x * inner.width(), inner.top() + y * inner.height());
    match (kind, ext) {
        (_, "md") => {
            p.text(at(0.36, 0.52), Align2::CENTER_CENTER, "M", semibold(side * 0.44), white);
            p.line_segment([at(0.82, 0.25), at(0.82, 0.75)], s);
            p.add(Shape::line(vec![at(0.66, 0.58), at(0.82, 0.76), at(0.98, 0.58)], s));
        }
        (_, "pdf") => {
            // stylised Acrobat-like swirl
            p.add(Shape::closed_line(vec![at(0.5, 0.1), at(0.15, 0.85), at(0.9, 0.65)], s));
        }
        (FileKind::Archive, _) => {
            for i in 0..4 {
                let y = 0.05 + i as f32 * 0.2;
                let x = if i % 2 == 0 { 0.38 } else { 0.5 };
                p.rect_filled(Rect::from_min_size(at(x, y), vec2(inner.width() * 0.14, inner.height() * 0.12)), 1.0, white);
            }
            p.rect_stroke(Rect::from_min_max(at(0.36, 0.78), at(0.64, 1.0)), 1.0, s);
        }
        (FileKind::Image, _) => glyph(p, inner.expand(side * 0.05), Glyph::Image, white),
        (FileKind::Video, _) => {
            p.add(Shape::convex_polygon(vec![at(0.28, 0.12), at(0.9, 0.5), at(0.28, 0.88)], white, Stroke::NONE));
        }
        (FileKind::Audio, _) => glyph(p, inner.expand(side * 0.05), Glyph::Music, white),
        (FileKind::Code, _) => {
            p.add(Shape::line(vec![at(0.3, 0.18), at(0.02, 0.5), at(0.3, 0.82)], s));
            p.add(Shape::line(vec![at(0.7, 0.18), at(0.98, 0.5), at(0.7, 0.82)], s));
        }
        (FileKind::Executable, _) => glyph(p, inner.expand(side * 0.05), Glyph::Gear, white),
        _ => glyph(p, inner.expand(side * 0.08), Glyph::Document, white),
    }
}

/// ".." entry: soft blue tile with an up arrow.
pub fn parent(p: &Painter, r: Rect) {
    let side = r.height().min(r.width());
    let t = Rect::from_center_size(r.center(), vec2(side, side));
    p.rect_filled(t, Rounding::same(side * 0.24), BLUE_SOFT);
    glyph(p, t.shrink(side * 0.16), Glyph::ArrowUp, BLUE);
}

/// Drive tile for the "This PC" view.
pub fn drive(p: &Painter, r: Rect) {
    let side = r.height().min(r.width());
    let t = Rect::from_center_size(r.center(), vec2(side, side));
    p.rect_filled(t, Rounding::same(side * 0.24), mix(SLATE, Color32::WHITE, 0.85));
    glyph(p, t.shrink(side * 0.14), Glyph::Drive, SLATE);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Glyph {
    Search,
    Home,
    Desktop,
    Document,
    Download,
    Image,
    Music,
    Video,
    Drive,
    Eye,
    Trash,
    Pencil,
    Plus,
    Copy,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Minus,
    Refresh,
    Columns,
    Gear,
    FolderPlus,
    ChevronDown,
    ChevronRight,
    Close,
    Question,
    Folder,
    Star,
    Heart,
    Bookmark,
    Cloud,
    Code,
    Briefcase,
    Book,
    Archive,
    Game,
    Scissors,
    Clipboard,
    Check,
    Unpack,
    Info,
    Terminal,
}

impl Glyph {
    /// Icons the user can pick for a sidebar entry, with their config keys.
    pub const PICKABLE: [(Glyph, &'static str); 22] = [
        (Glyph::Folder, "folder"),
        (Glyph::Home, "home"),
        (Glyph::Desktop, "pc"),
        (Glyph::Columns, "desktop"),
        (Glyph::Document, "document"),
        (Glyph::Download, "download"),
        (Glyph::Image, "image"),
        (Glyph::Music, "music"),
        (Glyph::Video, "video"),
        (Glyph::Drive, "drive"),
        (Glyph::Star, "star"),
        (Glyph::Heart, "heart"),
        (Glyph::Bookmark, "bookmark"),
        (Glyph::Cloud, "cloud"),
        (Glyph::Code, "code"),
        (Glyph::Briefcase, "work"),
        (Glyph::Book, "book"),
        (Glyph::Archive, "archive"),
        (Glyph::Game, "game"),
        (Glyph::Gear, "gear"),
        (Glyph::Trash, "trash"),
        (Glyph::Copy, "copy"),
    ];

    pub fn key(self) -> &'static str {
        Self::PICKABLE.iter().find(|(g, _)| *g == self).map(|(_, k)| *k).unwrap_or("folder")
    }

    pub fn from_key(k: &str) -> Glyph {
        Self::PICKABLE.iter().find(|(_, key)| *key == k).map(|(g, _)| *g).unwrap_or(Glyph::Folder)
    }
}

/// Outline line-icons for the sidebar, toolbars and dialogs.
pub fn glyph(p: &Painter, r: Rect, g: Glyph, c: Color32) {
    let s = Stroke::new((r.width() * 0.085).clamp(1.2, 2.0), c);
    let at = |x: f32, y: f32| pos2(r.left() + x * r.width(), r.top() + y * r.height());
    let w = r.width();
    match g {
        Glyph::Search => {
            p.circle_stroke(at(0.44, 0.44), w * 0.25, s);
            p.line_segment([at(0.63, 0.63), at(0.84, 0.84)], s);
        }
        Glyph::Home => {
            p.add(Shape::line(vec![at(0.14, 0.5), at(0.5, 0.18), at(0.86, 0.5)], s));
            p.add(Shape::line(vec![at(0.26, 0.4), at(0.26, 0.82), at(0.74, 0.82), at(0.74, 0.4)], s));
            p.add(Shape::line(vec![at(0.43, 0.82), at(0.43, 0.6), at(0.57, 0.6), at(0.57, 0.82)], s));
        }
        Glyph::Desktop => {
            p.rect_stroke(Rect::from_min_max(at(0.12, 0.2), at(0.88, 0.66)), 2.0, s);
            p.line_segment([at(0.5, 0.66), at(0.5, 0.82)], s);
            p.line_segment([at(0.32, 0.82), at(0.68, 0.82)], s);
        }
        Glyph::Document => {
            p.add(Shape::closed_line(
                vec![at(0.24, 0.12), at(0.58, 0.12), at(0.78, 0.32), at(0.78, 0.88), at(0.24, 0.88)],
                s,
            ));
            for y in [0.46, 0.6, 0.74] {
                p.line_segment([at(0.36, y), at(0.66, y)], s);
            }
        }
        Glyph::Download => {
            p.line_segment([at(0.5, 0.16), at(0.5, 0.64)], s);
            p.add(Shape::line(vec![at(0.31, 0.46), at(0.5, 0.65), at(0.69, 0.46)], s));
            p.line_segment([at(0.2, 0.84), at(0.8, 0.84)], s);
        }
        Glyph::Image => {
            p.rect_stroke(Rect::from_min_max(at(0.12, 0.18), at(0.88, 0.82)), 2.0, s);
            p.add(Shape::line(
                vec![at(0.18, 0.76), at(0.4, 0.52), at(0.56, 0.66), at(0.67, 0.56), at(0.84, 0.72)],
                s,
            ));
            p.circle_filled(at(0.66, 0.37), w * 0.07, c);
        }
        Glyph::Music => {
            p.line_segment([at(0.4, 0.74), at(0.4, 0.22)], s);
            p.line_segment([at(0.4, 0.22), at(0.76, 0.15)], s);
            p.line_segment([at(0.76, 0.15), at(0.76, 0.66)], s);
            p.circle_filled(at(0.32, 0.75), w * 0.1, c);
            p.circle_filled(at(0.68, 0.67), w * 0.1, c);
        }
        Glyph::Video => {
            p.rect_stroke(Rect::from_min_max(at(0.1, 0.28), at(0.64, 0.72)), 2.0, s);
            p.add(Shape::closed_line(vec![at(0.64, 0.44), at(0.9, 0.3), at(0.9, 0.7), at(0.64, 0.56)], s));
        }
        Glyph::Drive => {
            p.rect_stroke(Rect::from_min_max(at(0.12, 0.22), at(0.88, 0.78)), 3.0, s);
            p.line_segment([at(0.12, 0.56), at(0.88, 0.56)], s);
            p.circle_filled(at(0.72, 0.67), w * 0.05, c);
        }
        Glyph::Eye => {
            let n = 14;
            let mut pts = Vec::with_capacity(2 * n + 2);
            for i in 0..=n {
                let t = i as f32 / n as f32;
                pts.push(at(0.1 + 0.8 * t, 0.5 - 0.24 * (PI * t).sin()));
            }
            for i in (1..n).rev() {
                let t = i as f32 / n as f32;
                pts.push(at(0.1 + 0.8 * t, 0.5 + 0.24 * (PI * t).sin()));
            }
            p.add(Shape::closed_line(pts, s));
            p.circle_stroke(r.center(), w * 0.11, s);
        }
        Glyph::Trash => {
            p.line_segment([at(0.18, 0.28), at(0.82, 0.28)], s);
            p.add(Shape::line(vec![at(0.4, 0.28), at(0.4, 0.16), at(0.6, 0.16), at(0.6, 0.28)], s));
            p.add(Shape::line(vec![at(0.26, 0.28), at(0.3, 0.86), at(0.7, 0.86), at(0.74, 0.28)], s));
            p.line_segment([at(0.43, 0.42), at(0.43, 0.72)], s);
            p.line_segment([at(0.57, 0.42), at(0.57, 0.72)], s);
        }
        Glyph::Pencil => {
            p.add(Shape::closed_line(
                vec![at(0.2, 0.8), at(0.24, 0.62), at(0.66, 0.2), at(0.8, 0.34), at(0.38, 0.76)],
                s,
            ));
            p.line_segment([at(0.58, 0.28), at(0.72, 0.42)], s);
        }
        Glyph::Plus => {
            p.line_segment([at(0.5, 0.22), at(0.5, 0.78)], s);
            p.line_segment([at(0.22, 0.5), at(0.78, 0.5)], s);
        }
        Glyph::Copy => {
            p.rect_stroke(Rect::from_min_max(at(0.34, 0.34), at(0.84, 0.84)), 2.0, s);
            p.add(Shape::line(
                vec![at(0.66, 0.34), at(0.66, 0.16), at(0.16, 0.16), at(0.16, 0.66), at(0.34, 0.66)],
                s,
            ));
        }
        Glyph::ArrowLeft => {
            p.line_segment([at(0.2, 0.5), at(0.82, 0.5)], s);
            p.add(Shape::line(vec![at(0.42, 0.28), at(0.2, 0.5), at(0.42, 0.72)], s));
        }
        Glyph::ArrowRight => {
            p.line_segment([at(0.18, 0.5), at(0.8, 0.5)], s);
            p.add(Shape::line(vec![at(0.58, 0.28), at(0.8, 0.5), at(0.58, 0.72)], s));
        }
        Glyph::ArrowUp => {
            p.line_segment([at(0.5, 0.18), at(0.5, 0.82)], s);
            p.add(Shape::line(vec![at(0.28, 0.4), at(0.5, 0.18), at(0.72, 0.4)], s));
        }
        Glyph::ArrowDown => {
            p.line_segment([at(0.5, 0.18), at(0.5, 0.82)], s);
            p.add(Shape::line(vec![at(0.28, 0.6), at(0.5, 0.82), at(0.72, 0.6)], s));
        }
        Glyph::Minus => {
            p.line_segment([at(0.22, 0.5), at(0.78, 0.5)], s);
        }
        Glyph::Refresh => {
            let n = 20;
            let pts: Vec<_> = (0..=n)
                .map(|i| {
                    let a = PI * 0.35 + (PI * 1.6) * i as f32 / n as f32;
                    let rr = w * 0.3;
                    pos2(r.center().x + rr * a.cos(), r.center().y - rr * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s));
            p.add(Shape::line(vec![at(0.62, 0.14), at(0.72, 0.28), at(0.56, 0.36)], s));
        }
        Glyph::Columns => {
            p.rect_stroke(Rect::from_min_max(at(0.14, 0.2), at(0.86, 0.8)), 2.5, s);
            p.line_segment([at(0.5, 0.2), at(0.5, 0.8)], s);
        }
        Glyph::Gear => {
            let c0 = r.center();
            for i in 0..8 {
                let a = i as f32 * PI / 4.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c0 + d * w * 0.26, c0 + d * w * 0.4], Stroke::new(s.width * 1.6, c));
            }
            p.circle_stroke(c0, w * 0.27, s);
            p.circle_stroke(c0, w * 0.1, s);
        }
        Glyph::FolderPlus => {
            // folder outline, open at the bottom-right corner where the "+" badge sits
            p.add(Shape::line(
                vec![
                    at(0.86, 0.5),
                    at(0.86, 0.32),
                    at(0.48, 0.32),
                    at(0.4, 0.2),
                    at(0.1, 0.2),
                    at(0.1, 0.8),
                    at(0.54, 0.8),
                ],
                s,
            ));
            p.line_segment([at(0.78, 0.6), at(0.78, 0.96)], s);
            p.line_segment([at(0.6, 0.78), at(0.96, 0.78)], s);
        }
        Glyph::Folder => {
            p.add(Shape::closed_line(
                vec![at(0.1, 0.2), at(0.4, 0.2), at(0.48, 0.32), at(0.9, 0.32), at(0.9, 0.8), at(0.1, 0.8)],
                s,
            ));
            p.line_segment([at(0.1, 0.42), at(0.9, 0.42)], s);
        }
        Glyph::Star => {
            let c0 = r.center() + vec2(0.0, w * 0.03);
            let pts: Vec<_> = (0..10)
                .map(|i| {
                    let a = -PI / 2.0 + i as f32 * PI / 5.0;
                    let rr = if i % 2 == 0 { w * 0.4 } else { w * 0.17 };
                    c0 + vec2(a.cos(), a.sin()) * rr
                })
                .collect();
            p.add(Shape::closed_line(pts, s));
        }
        Glyph::Heart => {
            // classic parametric heart curve, scaled into the icon box
            let n = 40;
            let k = w * 0.024;
            let c0 = r.center() + vec2(0.0, -w * 0.04);
            let pts: Vec<_> = (0..n)
                .map(|i| {
                    let t = 2.0 * PI * i as f32 / n as f32;
                    let x = 16.0 * t.sin().powi(3);
                    let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
                    c0 + vec2(x * k, -y * k)
                })
                .collect();
            p.add(Shape::closed_line(pts, s));
        }
        Glyph::Bookmark => {
            p.add(Shape::closed_line(vec![at(0.26, 0.12), at(0.74, 0.12), at(0.74, 0.88), at(0.5, 0.68), at(0.26, 0.88)], s));
        }
        Glyph::Cloud => {
            let n = 10;
            let mut pts = Vec::new();
            let arc = |pts: &mut Vec<_>, cx: f32, cy: f32, rr: f32, a0: f32, a1: f32| {
                for i in 0..=n {
                    let a = a0 + (a1 - a0) * i as f32 / n as f32;
                    pts.push(pos2(r.left() + w * (cx + rr * a.cos()), r.top() + w * (cy - rr * a.sin())));
                }
            };
            arc(&mut pts, 0.28, 0.58, 0.14, PI * 1.5, PI * 0.6);
            arc(&mut pts, 0.5, 0.44, 0.2, PI * 0.95, PI * 0.15);
            arc(&mut pts, 0.74, 0.6, 0.12, PI * 0.6, -PI * 0.5);
            p.add(Shape::closed_line(pts, s));
        }
        Glyph::Code => {
            p.add(Shape::line(vec![at(0.34, 0.26), at(0.1, 0.5), at(0.34, 0.74)], s));
            p.add(Shape::line(vec![at(0.66, 0.26), at(0.9, 0.5), at(0.66, 0.74)], s));
            p.line_segment([at(0.56, 0.2), at(0.44, 0.8)], s);
        }
        Glyph::Briefcase => {
            p.rect_stroke(Rect::from_min_max(at(0.1, 0.3), at(0.9, 0.84)), 2.5, s);
            p.add(Shape::line(vec![at(0.36, 0.3), at(0.36, 0.16), at(0.64, 0.16), at(0.64, 0.3)], s));
            p.line_segment([at(0.1, 0.54), at(0.9, 0.54)], s);
        }
        Glyph::Book => {
            p.add(Shape::closed_line(vec![at(0.5, 0.26), at(0.12, 0.18), at(0.12, 0.8), at(0.5, 0.88), at(0.88, 0.8), at(0.88, 0.18)], s));
            p.line_segment([at(0.5, 0.26), at(0.5, 0.88)], s);
        }
        Glyph::Archive => {
            p.rect_stroke(Rect::from_min_max(at(0.1, 0.16), at(0.9, 0.36)), 2.0, s);
            p.add(Shape::line(vec![at(0.16, 0.36), at(0.16, 0.86), at(0.84, 0.86), at(0.84, 0.36)], s));
            p.line_segment([at(0.4, 0.52), at(0.6, 0.52)], s);
        }
        Glyph::Game => {
            p.rect_stroke(Rect::from_min_max(at(0.08, 0.28), at(0.92, 0.74)), w * 0.2, s);
            p.line_segment([at(0.3, 0.42), at(0.3, 0.6)], s);
            p.line_segment([at(0.21, 0.51), at(0.39, 0.51)], s);
            p.circle_filled(at(0.66, 0.46), w * 0.045, c);
            p.circle_filled(at(0.76, 0.56), w * 0.045, c);
        }
        Glyph::Scissors => {
            p.circle_stroke(at(0.3, 0.74), w * 0.12, s);
            p.circle_stroke(at(0.7, 0.74), w * 0.12, s);
            p.line_segment([at(0.37, 0.64), at(0.7, 0.14)], s);
            p.line_segment([at(0.63, 0.64), at(0.3, 0.14)], s);
        }
        Glyph::Clipboard => {
            p.rect_stroke(Rect::from_min_max(at(0.2, 0.2), at(0.8, 0.88)), 2.0, s);
            p.rect_stroke(Rect::from_min_max(at(0.36, 0.1), at(0.64, 0.28)), 1.5, s);
            p.line_segment([at(0.34, 0.5), at(0.66, 0.5)], s);
            p.line_segment([at(0.34, 0.66), at(0.58, 0.66)], s);
        }
        Glyph::Check => {
            p.add(Shape::line(vec![at(0.2, 0.52), at(0.42, 0.72), at(0.8, 0.3)], s));
        }
        Glyph::Unpack => {
            p.add(Shape::line(vec![at(0.14, 0.42), at(0.14, 0.86), at(0.86, 0.86), at(0.86, 0.42)], s));
            p.line_segment([at(0.5, 0.12), at(0.5, 0.62)], s);
            p.add(Shape::line(vec![at(0.32, 0.45), at(0.5, 0.63), at(0.68, 0.45)], s));
        }
        Glyph::ChevronDown => {
            p.add(Shape::line(vec![at(0.28, 0.4), at(0.5, 0.62), at(0.72, 0.4)], s));
        }
        Glyph::ChevronRight => {
            p.add(Shape::line(vec![at(0.38, 0.22), at(0.64, 0.5), at(0.38, 0.78)], s));
        }
        Glyph::Close => {
            p.line_segment([at(0.28, 0.28), at(0.72, 0.72)], s);
            p.line_segment([at(0.72, 0.28), at(0.28, 0.72)], s);
        }
        Glyph::Question => {
            p.circle_stroke(r.center(), w * 0.38, s);
            p.add(Shape::line(
                vec![at(0.4, 0.4), at(0.43, 0.31), at(0.5, 0.28), at(0.57, 0.31), at(0.6, 0.39), at(0.5, 0.5), at(0.5, 0.58)],
                s,
            ));
            p.circle_filled(at(0.5, 0.7), w * 0.045, c);
        }
        Glyph::Info => {
            p.circle_stroke(r.center(), w * 0.38, s);
            p.circle_filled(at(0.5, 0.32), w * 0.045, c);
            p.line_segment([at(0.5, 0.44), at(0.5, 0.7)], s);
        }
        Glyph::Terminal => {
            p.rect_stroke(Rect::from_min_max(at(0.1, 0.18), at(0.9, 0.82)), 2.5, s);
            p.add(Shape::line(vec![at(0.26, 0.38), at(0.42, 0.52), at(0.26, 0.66)], s));
            p.line_segment([at(0.5, 0.66), at(0.72, 0.66)], s);
        }
    }
}

/// Thin rounded progress bar.
pub fn bar(p: &Painter, r: Rect, frac: f32, color: Color32) {
    p.rect_filled(r, Rounding::same(r.height() / 2.0), TRACK);
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let w = (r.width() * f).max(r.height());
        p.rect_filled(Rect::from_min_size(r.min, vec2(w, r.height())), Rounding::same(r.height() / 2.0), color);
    }
}
