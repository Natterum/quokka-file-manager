//! UI helpers on top of the domain formatting: colours of file kinds, colour mixing.

use crate::theme::*;
use eframe::egui::Color32;
pub use quokka_domain::format::{format_size, format_time, items_text};
pub use quokka_domain::FileKind;

/// Colour of a file kind in icons and labels.
pub trait KindColor {
    fn color(self) -> Color32;
}

impl KindColor for FileKind {
    fn color(self) -> Color32 {
        match self {
            FileKind::Folder => YELLOW,
            FileKind::Document => BLUE,
            FileKind::Image => TEAL,
            FileKind::Video => PURPLE,
            FileKind::Audio => ORANGE,
            FileKind::Archive => RED,
            FileKind::Code => GREEN,
            FileKind::Executable => SLATE,
            FileKind::Other => GRAY,
        }
    }
}

/// Linear blend: t = 0 → a, t = 1 → b.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

