//! The Quokka logo (assets/quokka.svg, recoloured to the app palette) rasterised with resvg.

use eframe::egui;
use resvg::{tiny_skia, usvg};

const SVG: &str = include_str!("../assets/quokka.svg");

/// Premultiplied RGBA of the logo centred in a `size`×`size` square.
fn rasterize(size: u32) -> Option<tiny_skia::Pixmap> {
    let tree = usvg::Tree::from_str(SVG, &usvg::Options::default()).ok()?;
    let sz = tree.size();
    let scale = size as f32 / sz.width().max(sz.height());
    let tx = (size as f32 - sz.width() * scale) / 2.0;
    let ty = (size as f32 - sz.height() * scale) / 2.0;
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, tx, ty), &mut pixmap.as_mut());
    Some(pixmap)
}

pub fn icon_data(size: u32) -> egui::IconData {
    let rgba = match rasterize(size) {
        Some(p) => p
            .pixels()
            .iter()
            .flat_map(|c| {
                let c = c.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect(),
        None => vec![0; (size * size * 4) as usize],
    };
    egui::IconData { rgba, width: size, height: size }
}

pub fn texture(ctx: &egui::Context, size: u32) -> Option<egui::TextureHandle> {
    let p = rasterize(size)?;
    let img = egui::ColorImage::from_rgba_premultiplied([size as usize, size as usize], p.data());
    Some(ctx.load_texture("quokka-logo", img, egui::TextureOptions::LINEAR))
}
