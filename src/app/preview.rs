//! Background text and photo viewer, also for files inside ZIP folders.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};
use std::time::Duration;

enum Content {
    Text(String),
    Photo {
        pixels: egui::ColorImage,
        dimensions: (u32, u32),
    },
}

pub(super) struct Preview {
    path: PathBuf,
    rx: Receiver<Result<(Content, String), String>>,
    cancel: Arc<AtomicBool>,
    text: Option<String>,
    texture: Option<egui::TextureHandle>,
    info: String,
    loading: bool,
    error: Option<String>,
    fit: bool,
    zoom: f32,
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Preview {
    pub(super) fn new(path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let file = path.clone();
        let spawned = std::thread::Builder::new()
            .name("quokka-preview".into())
            .spawn(move || {
                let result = load(&file, &flag).map_err(|e| e.to_string());
                if !flag.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                }
            });
        let error = spawned.err().map(|e| e.to_string());
        let loading = error.is_none();
        Self {
            path,
            rx,
            cancel,
            text: None,
            texture: None,
            info: String::new(),
            loading,
            error,
            fit: true,
            zoom: 1.0,
        }
    }

    pub(super) fn show(&mut self, ctx: &egui::Context, esc: bool) -> bool {
        if self.loading {
            match self.rx.try_recv() {
                Ok(Ok((content, info))) => {
                    self.info = info;
                    match content {
                        Content::Text(text) => self.text = Some(text),
                        Content::Photo { pixels, dimensions } => {
                            self.info =
                                format!("{} × {} · {}", dimensions.0, dimensions.1, self.info);
                            self.texture = Some(ctx.load_texture(
                                "photo-preview",
                                pixels,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                    }
                    self.loading = false;
                }
                Ok(Err(e)) => {
                    self.error = Some(e);
                    self.loading = false;
                }
                Err(TryRecvError::Disconnected) => {
                    self.error = Some(tr!("Просмотр прерван", "Preview was interrupted").into());
                    self.loading = false;
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(40)),
            }
        }
        let mut close = esc || ctx.input(|i| i.key_pressed(Key::F3) || i.key_pressed(Key::F10));
        let screen = ctx.screen_rect();
        let name = self.path.file_name().unwrap_or_default().to_string_lossy();
        let width = (screen.width() * 0.85).min(1200.0);
        dialog(ctx, "viewer", width, |ui| {
            dialog_header(ui, Glyph::Eye, BLUE, &name, Some(&self.info));
            if self.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr!("Загрузка…", "Loading…"));
                });
                ui.add_space(80.0);
            } else if let Some(error) = &self.error {
                ui.colored_label(RED, error);
            } else if let Some(texture) = &self.texture {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.fit, true, tr!("Вписать", "Fit"));
                    if ui
                        .selectable_label(!self.fit && self.zoom == 1.0, "100%")
                        .clicked()
                    {
                        self.fit = false;
                        self.zoom = 1.0;
                    }
                    if ui.button("−").clicked() {
                        self.fit = false;
                        self.zoom = (self.zoom / 1.25).max(0.1);
                    }
                    if ui.button("+").clicked() {
                        self.fit = false;
                        self.zoom = (self.zoom * 1.25).min(4.0);
                    }
                });
                let available = vec2(
                    ui.available_width().max(1.0),
                    (screen.height() * 0.58).max(80.0),
                );
                let native = texture.size_vec2();
                let size = if self.fit {
                    native
                        * (available.x / native.x)
                            .min(available.y / native.y)
                            .min(1.0)
                } else {
                    native * self.zoom
                };
                egui::Frame::none()
                    .fill(INPUT_BG)
                    .rounding(8.0)
                    .show(ui, |ui| {
                        egui::ScrollArea::both()
                            .id_salt("photo-scroll")
                            .max_height(available.y)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.add(egui::Image::new((texture.id(), size)));
                            });
                    });
            } else if let Some(text) = &self.text {
                egui::ScrollArea::both()
                    .id_salt("text-preview")
                    .max_height(screen.height() * 0.62)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut value = text.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut value)
                                .font(mono(13.0))
                                .desired_width(f32::INFINITY)
                                .frame(false),
                        );
                    });
            }
            ui.add_space(12.0);
            buttons_row(ui, |ui| {
                if btn(ui, tr!("Закрыть", "Close"), BLUE, Color32::WHITE).clicked() {
                    close = true;
                }
                ui.label(
                    RichText::new(tr!("F3 / Esc — закрыть", "F3 / Esc — close"))
                        .color(MUTED)
                        .size(12.5),
                );
            });
        });
        !close
    }
}

fn load(
    path: &std::path::Path,
    cancel: &AtomicBool,
) -> Result<(Content, String), Box<dyn std::error::Error>> {
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let photo = matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" | "ico"
    );
    if !photo && !is_text_extension(&ext) {
        return Err(unsupported_text_preview().into());
    }
    let limit = if photo { 64 * 1024 * 1024 } else { 512 * 1024 };
    let (data, total) = quokka_infra::archive::read(path, limit, !photo, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(std::io::Error::from(std::io::ErrorKind::Interrupted).into());
    }
    if photo {
        use image::ImageDecoder;
        let mut reader =
            image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(256 * 1024 * 1024);
        limits.max_image_width = Some(16384);
        limits.max_image_height = Some(16384);
        reader.limits(limits);
        let mut decoder = reader.into_decoder()?;
        let orientation = decoder.orientation().ok();
        let mut image = image::DynamicImage::from_decoder(decoder)?;
        if let Some(orientation) = orientation {
            image.apply_orientation(orientation);
        }
        let dimensions = (image.width(), image.height());
        // Bound GPU memory and texture dimensions independently of decoder limits.
        let reduced = dimensions.0 > 4096 || dimensions.1 > 4096;
        if reduced {
            image = image.thumbnail(4096, 4096);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::from(std::io::ErrorKind::Interrupted).into());
        }
        let rgba = image.into_rgba8();
        let pixels = egui::ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        );
        let info = format!(
            "{}{}",
            format_size(total),
            if reduced {
                tr!(" · уменьшено до 4096 px", " · reduced to 4096 px")
            } else {
                ""
            }
        );
        Ok((Content::Photo { pixels, dimensions }, info))
    } else {
        let text = preview_text(&data, total > data.len() as u64)?;
        let info = format!(
            "{} · {}{}",
            format_size(total),
            tr!("текст", "text"),
            if total > limit as u64 {
                tr!(" · показано начало", " · showing the beginning")
            } else {
                ""
            }
        );
        Ok((Content::Text(text.to_owned()), info))
    }
}

fn unsupported_text_preview() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        tr!(
            "Невозможно показать файл как текст: нетекстовый или неподдерживаемый формат",
            "Cannot preview this file as text: non-text or unsupported format"
        ),
    )
}

fn is_text_extension(ext: &str) -> bool {
    matches!(
        ext,
        "" | "txt"
            | "text"
            | "log"
            | "md"
            | "markdown"
            | "rst"
            | "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "xml"
            | "html"
            | "htm"
            | "svg"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "conf"
            | "config"
            | "properties"
            | "env"
            | "gitignore"
            | "gitattributes"
            | "editorconfig"
            | "rs"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cc"
            | "cxx"
            | "cs"
            | "java"
            | "kt"
            | "kts"
            | "go"
            | "py"
            | "pyw"
            | "rb"
            | "php"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "vue"
            | "svelte"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "ps1"
            | "psm1"
            | "psd1"
            | "bat"
            | "cmd"
            | "sql"
            | "lua"
            | "r"
            | "swift"
            | "dart"
            | "ex"
            | "exs"
            | "erl"
            | "hrl"
            | "hs"
            | "pl"
            | "pm"
            | "cmake"
            | "mk"
            | "gradle"
            | "tex"
            | "bib"
            | "srt"
            | "vtt"
            | "diff"
            | "patch"
            | "lock"
    )
}

fn preview_text(data: &[u8], truncated: bool) -> std::io::Result<&str> {
    let text = match std::str::from_utf8(data) {
        Ok(text) => text,
        // A bounded read may split the last UTF-8 character.
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&data[..error.valid_up_to()])
                .map_err(|_| unsupported_text_preview())?
        }
        Err(_) => return Err(unsupported_text_preview()),
    };
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}'))
    {
        return Err(unsupported_text_preview());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn previews_photos_from_disk_and_zip_and_reports_invalid_images() {
        let root = std::env::temp_dir().join(format!("quokka-preview-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("photo.png");
        image::RgbaImage::from_pixel(32, 16, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let cancel = AtomicBool::new(false);
        match load(&path, &cancel).unwrap().0 {
            Content::Photo { pixels, dimensions } => {
                assert_eq!(dimensions, (32, 16));
                assert_eq!(pixels.size, [32, 16]);
            }
            _ => panic!("expected photo"),
        }
        let archive = root.join("photos.zip");
        let mut writer = quokka_infra::zip::ZipWriter::create(&archive).unwrap();
        writer
            .add_file(
                "photos/photo.png",
                &mut fs::File::open(&path).unwrap(),
                None,
                |_| Ok(()),
            )
            .unwrap();
        writer.finish().unwrap();
        assert!(matches!(
            load(&archive.join("photos/photo.png"), &cancel).unwrap().0,
            Content::Photo {
                dimensions: (32, 16),
                ..
            }
        ));
        fs::write(&path, b"invalid image").unwrap();
        assert!(load(&path, &cancel).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn text_binary_and_cancelled_previews() {
        let path =
            std::env::temp_dir().join(format!("quokka-preview-text-{}.txt", std::process::id()));
        let cancel = AtomicBool::new(false);
        fs::write(&path, "Текст").unwrap();
        assert!(matches!(load(&path, &cancel).unwrap().0, Content::Text(text) if text == "Текст"));
        fs::write(&path, b"\0abc").unwrap();
        assert!(load(&path, &cancel).is_err());
        cancel.store(true, Ordering::Relaxed);
        assert!(load(&path, &cancel).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_archives_and_unsupported_files_on_disk_and_in_zip() {
        let root =
            std::env::temp_dir().join(format!("quokka-preview-types-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let cancel = AtomicBool::new(false);
        for name in [
            "archive.ZIP",
            "archive.7z",
            "archive.tar",
            "document.pdf",
            "document.docx",
            "app.exe",
            "music.mp3",
            "unknown.xyz",
        ] {
            let path = root.join(name);
            // Unsupported formats must be rejected even with text-like or empty content.
            fs::write(&path, b"plain text").unwrap();
            assert!(load(&path, &cancel).is_err(), "{name}");
        }
        let archive = root.join("files.zip");
        let mut writer = quokka_infra::zip::ZipWriter::create(&archive).unwrap();
        for (name, data) in [
            ("nested.zip", b"plain text".as_slice()),
            ("binary.txt", b"abc\0def".as_slice()),
            ("readme.txt", "Текст".as_bytes()),
        ] {
            writer
                .add_file(name, &mut std::io::Cursor::new(data), None, |_| Ok(()))
                .unwrap();
        }
        writer.finish().unwrap();
        assert!(load(&archive, &cancel).is_err());
        assert!(load(&archive.join("nested.zip"), &cancel).is_err());
        assert!(load(&archive.join("binary.txt"), &cancel).is_err());
        assert!(
            matches!(load(&archive.join("readme.txt"), &cancel).unwrap().0, Content::Text(text) if text == "Текст")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_all_text_bytes_and_handles_utf8_at_preview_limit() {
        assert_eq!(
            preview_text("Текст\n\t".as_bytes(), false).unwrap(),
            "Текст\n\t"
        );
        assert_eq!(preview_text(b"", false).unwrap(), "");
        assert!(preview_text(b"text\xff", false).is_err());
        assert!(preview_text(b"text\x01", false).is_err());
        let mut data = vec![b'a'; 8192];
        data.push(0);
        assert!(preview_text(&data, false).is_err());
        assert_eq!(preview_text(b"text\xd0", true).unwrap(), "text");
        assert!(preview_text(b"text\xd0", false).is_err());
        assert!(preview_text(b"text\xff", true).is_err());
    }
}
