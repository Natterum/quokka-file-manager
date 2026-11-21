//! Windows: render assets/quokka.svg into a multi-size .ico and embed it
//! as the executable's icon (shown in Explorer, the taskbar and shortcuts).

fn main() {
    println!("cargo:rerun-if-changed=assets/quokka.svg");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out.join("quokka.ico");
    write_ico(&ico);

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().unwrap());
    res.set("FileDescription", "Quokka File Manager");
    res.set("ProductName", "Quokka File Manager");
    res.compile().expect("failed to embed Windows resources");
}

fn write_ico(path: &std::path::Path) {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    use resvg::{tiny_skia, usvg};

    let svg = std::fs::read_to_string("assets/quokka.svg").unwrap();
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap();
    let sz = tree.size();
    let frames: Vec<IcoFrame> = [16u32, 20, 24, 32, 40, 48, 64, 128, 256]
        .iter()
        .map(|&size| {
            let scale = size as f32 / sz.width().max(sz.height());
            let tx = (size as f32 - sz.width() * scale) / 2.0;
            let ty = (size as f32 - sz.height() * scale) / 2.0;
            let mut pm = tiny_skia::Pixmap::new(size, size).unwrap();
            resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, tx, ty), &mut pm.as_mut());
            let rgba: Vec<u8> = pm
                .pixels()
                .iter()
                .flat_map(|c| {
                    let c = c.demultiply();
                    [c.red(), c.green(), c.blue(), c.alpha()]
                })
                .collect();
            IcoFrame::as_png(&rgba, size, size, image::ExtendedColorType::Rgba8).unwrap()
        })
        .collect();
    let file = std::fs::File::create(path).unwrap();
    IcoEncoder::new(file).encode_images(&frames).unwrap();
}
