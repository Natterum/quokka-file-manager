//! Batch image conversion: target formats, quality settings and where each result
//! goes. The conversion itself lives in `quokka_infra::imgconv`.

use crate::tr;
use std::path::{Path, PathBuf};

/// Format an image can be converted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Webp,
    Avif,
    Bmp,
    Tiff,
    Gif,
    Ico,
    Tga,
    Qoi,
}

impl ImageFormat {
    pub const ALL: [ImageFormat; 10] = [
        ImageFormat::Jpeg,
        ImageFormat::Png,
        ImageFormat::Webp,
        ImageFormat::Avif,
        ImageFormat::Bmp,
        ImageFormat::Tiff,
        ImageFormat::Gif,
        ImageFormat::Ico,
        ImageFormat::Tga,
        ImageFormat::Qoi,
    ];

    /// Extension of the result, without the dot.
    pub fn ext(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
            ImageFormat::Webp => "webp",
            ImageFormat::Avif => "avif",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Tiff => "tif",
            ImageFormat::Gif => "gif",
            ImageFormat::Ico => "ico",
            ImageFormat::Tga => "tga",
            ImageFormat::Qoi => "qoi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Png => "PNG",
            ImageFormat::Webp => "WebP",
            ImageFormat::Avif => "AVIF",
            ImageFormat::Bmp => "BMP",
            ImageFormat::Tiff => "TIFF",
            ImageFormat::Gif => "GIF",
            ImageFormat::Ico => "ICO",
            ImageFormat::Tga => "TGA",
            ImageFormat::Qoi => "QOI",
        }
    }

    /// One-line description for the format picker.
    pub fn hint(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => tr!("Фото, с потерями, без прозрачности", "Photos, lossy, no transparency"),
            ImageFormat::Png => tr!("Без потерь, прозрачность", "Lossless, transparency"),
            ImageFormat::Webp => tr!("С потерями или без, прозрачность", "Lossy or lossless, transparency"),
            ImageFormat::Avif => tr!("Самый компактный, медленное сжатие", "Smallest files, slow to encode"),
            ImageFormat::Bmp => tr!("Без сжатия", "Uncompressed"),
            ImageFormat::Tiff => tr!("Без потерь, для печати", "Lossless, for print"),
            ImageFormat::Gif => tr!("256 цветов", "256 colors"),
            ImageFormat::Ico => tr!("Значок, до 256×256", "Icon, up to 256×256"),
            ImageFormat::Tga => tr!("Без сжатия, прозрачность", "Uncompressed, transparency"),
            ImageFormat::Qoi => tr!("Без потерь, очень быстрый", "Lossless, very fast"),
        }
    }

    /// Whether a quality slider (1–100) applies.
    pub fn has_quality(self) -> bool {
        matches!(self, ImageFormat::Jpeg | ImageFormat::Webp | ImageFormat::Avif)
    }

    /// Whether the result can keep an alpha channel.
    pub fn has_alpha(self) -> bool {
        !matches!(self, ImageFormat::Jpeg)
    }

    /// Key stored in the settings file.
    pub fn key(self) -> &'static str {
        self.ext()
    }

    pub fn from_key(s: &str) -> Option<ImageFormat> {
        ImageFormat::ALL.into_iter().find(|f| f.key() == s)
    }
}

/// Compression effort for PNG (always lossless).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PngLevel {
    Fast,
    #[default]
    Default,
    Best,
}

impl PngLevel {
    pub const ALL: [PngLevel; 3] = [PngLevel::Fast, PngLevel::Default, PngLevel::Best];

    pub fn label(self) -> &'static str {
        match self {
            PngLevel::Fast => tr!("Быстро", "Fast"),
            PngLevel::Default => tr!("Обычно", "Default"),
            PngLevel::Best => tr!("Сильное сжатие", "Best compression"),
        }
    }
}

/// How to convert: target format and its settings.
#[derive(Clone, Debug, PartialEq)]
pub struct ConvertOptions {
    pub format: ImageFormat,
    /// 1–100, for formats with [`ImageFormat::has_quality`].
    pub quality: u8,
    /// WebP only: lossless instead of `quality`.
    pub lossless: bool,
    pub png: PngLevel,
    /// Replace existing files (otherwise they are skipped).
    pub overwrite: bool,
    /// Give results the modification time of their sources.
    pub keep_time: bool,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        ConvertOptions {
            format: ImageFormat::Webp,
            quality: 85,
            lossless: false,
            png: PngLevel::Default,
            overwrite: false,
            keep_time: true,
        }
    }
}

impl ConvertOptions {
    /// Quality clamped to 1–100.
    pub fn quality(&self) -> u8 {
        self.quality.clamp(1, 100)
    }

    /// Short description for titles: «WebP 85%», «PNG».
    pub fn describe(&self) -> String {
        let f = self.format;
        if f == ImageFormat::Webp && self.lossless {
            format!("{} {}", f.label(), tr!("без потерь", "lossless"))
        } else if f.has_quality() {
            format!("{} {}%", f.label(), self.quality())
        } else {
            f.label().to_string()
        }
    }
}

/// Extensions the converter can read (lower case, no dot).
pub const READABLE: [&str; 14] =
    ["jpg", "jpeg", "jfif", "png", "webp", "bmp", "dib", "gif", "ico", "tif", "tiff", "tga", "qoi", "apng"];

/// Whether `path` looks like an image the converter can read.
pub fn can_read(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| READABLE.contains(&e.as_str()))
}

/// Where `src` goes: `dest_dir/<rel parent>/<stem>.<ext>`, where `rel` is the path
/// of `src` relative to the selected item it was found under (empty for a file that
/// was selected itself).
pub fn target_path(src: &Path, rel_dir: &Path, dest_dir: &Path, format: ImageFormat) -> PathBuf {
    let stem = src.file_stem().map(|s| s.to_os_string()).unwrap_or_default();
    let mut name = stem;
    name.push(".");
    name.push(format.ext());
    dest_dir.join(rel_dir).join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_keeps_stem_and_subfolder() {
        let t = target_path(Path::new("/a/photos/x.y.PNG"), Path::new("photos"), Path::new("/out"), ImageFormat::Webp);
        assert_eq!(t, PathBuf::from("/out/photos/x.y.webp"));
    }

    #[test]
    fn readable_ignores_case() {
        assert!(can_read(Path::new("A.JPEG")));
        assert!(!can_read(Path::new("a.heic")));
        assert!(!can_read(Path::new("noext")));
    }

    #[test]
    fn format_keys_round_trip() {
        for f in ImageFormat::ALL {
            assert_eq!(ImageFormat::from_key(f.key()), Some(f));
        }
    }

    #[test]
    fn describe_shows_quality_only_where_it_applies() {
        let mut o = ConvertOptions { format: ImageFormat::Jpeg, quality: 0, ..Default::default() };
        assert_eq!(o.describe(), "JPEG 1%");
        o.format = ImageFormat::Png;
        assert_eq!(o.describe(), "PNG");
    }
}
