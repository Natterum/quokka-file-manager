//! Batch image conversion on the `image` crate (lossy WebP through libwebp). Files
//! are converted in parallel, one worker per core; one broken file does not stop
//! the batch. The rules (formats, target names) live in `quokka_domain::imgconv`.

use quokka_core::job::{Job, Reporter};
use quokka_core::Waker;
use quokka_domain::format::items_text;
use quokka_domain::imgconv::{can_read, target_path, ConvertOptions, ImageFormat, PngLevel};
use quokka_domain::{tr, trf};
use image::codecs::{avif, bmp, gif, ico, jpeg, png, qoi, tga, tiff, webp as iwebp};
use image::{DynamicImage, ImageDecoder, ImageReader, Rgba, RgbaImage};
use std::fs::{self, File};
use std::io::{BufWriter, Cursor, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

/// One file to convert: source and where the result goes.
#[derive(Debug, Clone, PartialEq)]
struct Item {
    src: PathBuf,
    dst: PathBuf,
}

/// Images under `sources` (folders recursively) with their targets in `dest`, plus the
/// number of selected files that are not readable images.
fn collect(sources: &[PathBuf], dest: &Path, format: ImageFormat) -> (Vec<Item>, usize) {
    fn walk(dir: &Path, rel: &Path, dest: &Path, format: ImageFormat, out: &mut Vec<Item>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                walk(&p, &rel.join(e.file_name()), dest, format, out);
            } else if can_read(&p) {
                out.push(Item { dst: target_path(&p, rel, dest, format), src: p });
            }
        }
    }
    let mut items = Vec::new();
    let mut unsupported = 0;
    for s in sources {
        if s.is_dir() {
            let rel = PathBuf::from(s.file_name().unwrap_or_default());
            walk(s, &rel, dest, format, &mut items);
        } else if can_read(s) {
            items.push(Item { dst: target_path(s, Path::new(""), dest, format), src: s.clone() });
        } else {
            unsupported += 1;
        }
    }
    (items, unsupported)
}

fn image_err(e: image::ImageError) -> String {
    match e {
        image::ImageError::Unsupported(u) => trf!("формат не поддерживается ({u})", "unsupported format ({u})"),
        e => e.to_string(),
    }
}

/// Decode `path`, turning it upright by its EXIF orientation (the tag itself is not
/// carried over, so the pixels must already be rotated).
fn load(path: &Path) -> Result<DynamicImage, String> {
    let reader = ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(image_err)?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(image_err)?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok(img)
}

/// Pixels on a white background, for formats without transparency.
fn flatten(img: &DynamicImage) -> DynamicImage {
    if !img.color().has_alpha() {
        return DynamicImage::ImageRgb8(img.to_rgb8());
    }
    let mut rgba = img.to_rgba8();
    for Rgba([r, g, b, a]) in rgba.pixels_mut() {
        let a16 = *a as u16;
        let mix = |c: u8| ((c as u16 * a16 + 255 * (255 - a16) + 127) / 255) as u8;
        (*r, *g, *b) = (mix(*r), mix(*g), mix(*b));
        *a = 255;
    }
    DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(rgba).to_rgb8())
}

/// 8 bits per channel, RGBA if the source has alpha, otherwise RGB.
fn to_8bit(img: &DynamicImage) -> DynamicImage {
    if img.color().has_alpha() {
        DynamicImage::ImageRgba8(img.to_rgba8())
    } else {
        DynamicImage::ImageRgb8(img.to_rgb8())
    }
}

/// Keep 8 / 16-bit integer pixels as they are (PNG and TIFF store both); floats go to 16 bits.
fn to_int(img: DynamicImage) -> DynamicImage {
    match img {
        DynamicImage::ImageRgb32F(_) => DynamicImage::ImageRgb16(img.to_rgb16()),
        DynamicImage::ImageRgba32F(_) => DynamicImage::ImageRgba16(img.to_rgba16()),
        other => other,
    }
}

/// Encode `img` as `o.format` into `w`.
fn encode<W: Write + Seek>(img: DynamicImage, o: &ConvertOptions, w: &mut W) -> Result<(), String> {
    let q = o.quality();
    let r = match o.format {
        ImageFormat::Jpeg => flatten(&img).write_with_encoder(jpeg::JpegEncoder::new_with_quality(&mut *w, q)),
        ImageFormat::Png => {
            let level = match o.png {
                PngLevel::Fast => png::CompressionType::Fast,
                PngLevel::Default => png::CompressionType::Default,
                PngLevel::Best => png::CompressionType::Best,
            };
            to_int(img).write_with_encoder(png::PngEncoder::new_with_quality(&mut *w, level, png::FilterType::Adaptive))
        }
        ImageFormat::Webp if o.lossless => to_8bit(&img).write_with_encoder(iwebp::WebPEncoder::new_lossless(&mut *w)),
        ImageFormat::Webp => {
            let rgba = img.to_rgba8();
            let (wd, ht) = rgba.dimensions();
            let encoded = ::webp::Encoder::from_rgba(&rgba, wd, ht).encode_simple(false, q as f32)
                .map_err(|e| trf!("ошибка WebP: {:?}", "WebP error: {:?}", e))?;
            return w.write_all(&encoded).map_err(|e| e.to_string());
        }
        ImageFormat::Avif => to_8bit(&img).write_with_encoder(avif::AvifEncoder::new_with_speed_quality(&mut *w, 6, q)),
        ImageFormat::Bmp => to_8bit(&img).write_with_encoder(bmp::BmpEncoder::new(&mut *w)),
        ImageFormat::Tiff => to_int(img).write_with_encoder(tiff::TiffEncoder::new(&mut *w)),
        ImageFormat::Tga => to_8bit(&img).write_with_encoder(tga::TgaEncoder::new(&mut *w)),
        ImageFormat::Qoi => to_8bit(&img).write_with_encoder(qoi::QoiEncoder::new(&mut *w)),
        ImageFormat::Ico => {
            let img = if img.width() > 256 || img.height() > 256 { img.thumbnail(256, 256) } else { img };
            DynamicImage::ImageRgba8(img.to_rgba8()).write_with_encoder(ico::IcoEncoder::new(&mut *w))
        }
        ImageFormat::Gif => {
            let rgba: RgbaImage = img.to_rgba8();
            let mut enc = gif::GifEncoder::new_with_speed(&mut *w, 10);
            enc.encode_frame(image::Frame::new(rgba))
        }
    };
    r.map_err(image_err)
}

enum Outcome {
    Converted,
    Skipped,
}

/// Convert one file. The result is written next to the target under a temporary name
/// and renamed at the end, so a failure never leaves a half-written file and the
/// source can be its own target (re-compressing JPEGs in place).
fn convert_one(item: &Item, o: &ConvertOptions) -> Result<Outcome, String> {
    if item.dst.exists() && !o.overwrite {
        return Ok(Outcome::Skipped);
    }
    let img = load(&item.src)?;
    let dir = item.dst.parent().unwrap_or(Path::new(""));
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(item.dst.file_name().unwrap_or_default());
    tmp_name.push(".quokka-tmp");
    let tmp = dir.join(tmp_name);
    let written = (|| {
        let mut w = BufWriter::new(File::create(&tmp).map_err(|e| e.to_string())?);
        encode(img, o, &mut w)?;
        let file = w.into_inner().map_err(|e| e.error().to_string())?;
        if o.keep_time {
            if let Ok(t) = fs::metadata(&item.src).and_then(|m| m.modified()) {
                let _ = file.set_modified(t);
            }
        }
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&tmp, &item.dst).map_err(|e| e.to_string())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written.map(|()| Outcome::Converted)
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

/// Run the whole batch, reporting progress in files.
fn run(sources: &[PathBuf], dest: &Path, o: &ConvertOptions, rep: &mut Reporter) -> Result<String, String> {
    let (items, unsupported) = collect(sources, dest, o.format);
    if items.is_empty() {
        return Err(tr!("Среди выбранного нет изображений, которые можно прочитать", "No readable images among the selected items").into());
    }
    rep.total = items.len() as u64;
    rep.tick(true);

    let workers = thread::available_parallelism().map_or(2, |n| n.get()).min(items.len()).max(1);
    let next = AtomicUsize::new(0);
    let cancel = rep.cancel.clone();
    let (tx, rx) = mpsc::channel::<(usize, Result<Outcome, String>)>();
    let (mut converted, mut skipped) = (0usize, 0usize);
    let mut errors: Vec<String> = Vec::new();
    thread::scope(|s| {
        for i in 0..workers {
            let tx = tx.clone();
            let (items, next, cancel) = (&items, &next, &cancel);
            // AVIF encoding (rav1e) wants more stack than the 2 MB default
            let spawned = thread::Builder::new().name(format!("quokka-img-{i}")).stack_size(16 * 1024 * 1024).spawn_scoped(s, move || loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(k) else { break };
                let r = std::panic::catch_unwind(|| convert_one(item, o))
                    .unwrap_or_else(|e| Err(quokka_core::job::panic_text(&e)));
                if tx.send((k, r)).is_err() {
                    break;
                }
            });
            if let Err(e) = spawned {
                errors.push(e.to_string());
            }
        }
        drop(tx);
        for (k, r) in rx {
            rep.done += 1;
            rep.current = name_of(&items[k].src);
            match r {
                Ok(Outcome::Converted) => converted += 1,
                Ok(Outcome::Skipped) => skipped += 1,
                Err(e) => errors.push(format!("{}: {e}", name_of(&items[k].src))),
            }
            rep.tick(false);
        }
    });
    rep.skipped = skipped;
    rep.tick(true);

    if cancel.load(Ordering::Relaxed) {
        return Err(trf!("Отменено. Сконвертировано: {}", "Cancelled. Converted: {}", items_text(converted as u64)));
    }
    let mut s = trf!("Сконвертировано: {}", "Converted: {}", items_text(converted as u64));
    if skipped > 0 {
        s.push_str(&trf!(", пропущено существующих: {skipped}", ", existing skipped: {skipped}"));
    }
    if unsupported > 0 {
        s.push_str(&trf!(", не изображения: {unsupported}", ", not images: {unsupported}"));
    }
    if errors.is_empty() {
        Ok(s)
    } else {
        let more = if errors.len() > 1 { trf!(" (и ещё {})", " (and {} more)", errors.len() - 1) } else { String::new() };
        Err(trf!("{s}. Ошибок: {}. {}{more}", "{s}. Errors: {}. {}{more}", errors.len(), errors[0]))
    }
}

pub fn start_convert(sources: Vec<PathBuf>, dest: PathBuf, options: ConvertOptions, waker: Waker) -> Job {
    let title = trf!("Конвертация в {}", "Converting to {}", options.describe());
    Job::spawn(title, false, waker, move |rep| run(&sources, &dest, &options, rep))
}

/// Expected size of the results, from converting a few sample files in memory.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    /// Images that will be converted.
    pub files: usize,
    /// Their total size now.
    pub input_bytes: u64,
    /// Expected total size after conversion.
    pub output_bytes: u64,
    /// How many files the estimate is based on.
    pub sampled: usize,
}

/// Files sampled for [`estimate`]: enough to smooth out one odd picture, few enough
/// to answer in about a second even for AVIF.
const SAMPLES: usize = 4;

/// Estimate the size of the results: convert up to [`SAMPLES`] files spread over the
/// selection in memory and scale their size ratio to all the images. `None` if
/// cancelled or no sample could be converted.
pub fn estimate(sources: &[PathBuf], o: &ConvertOptions, cancel: &AtomicBool) -> Option<Estimate> {
    let (items, _) = collect(sources, Path::new(""), o.format);
    let sizes: Vec<u64> = items.iter().map(|i| fs::metadata(&i.src).map_or(0, |m| m.len())).collect();
    let input_bytes: u64 = sizes.iter().sum();
    if items.is_empty() {
        return None;
    }
    let n = items.len().min(SAMPLES);
    let picks: Vec<usize> = (0..n).map(|k| k * items.len() / n).collect();
    let (mut sample_in, mut sample_out, mut sampled) = (0u64, 0u64, 0usize);
    let results: Vec<Option<(u64, u64)>> = thread::scope(|s| {
        let handles: Vec<_> = picks
            .iter()
            .map(|&k| {
                let (item, size) = (&items[k], sizes[k]);
                thread::Builder::new().stack_size(16 * 1024 * 1024).spawn_scoped(s, move || {
                    if cancel.load(Ordering::Relaxed) || size == 0 {
                        return None;
                    }
                    let img = load(&item.src).ok()?;
                    let mut buf = Cursor::new(Vec::new());
                    encode(img, o, &mut buf).ok()?;
                    Some((size, buf.into_inner().len() as u64))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.ok().and_then(|h| h.join().ok().flatten())).collect()
    });
    for (i, o) in results.into_iter().flatten() {
        sample_in += i;
        sample_out += o;
        sampled += 1;
    }
    if cancel.load(Ordering::Relaxed) || sampled == 0 || sample_in == 0 {
        return None;
    }
    let output_bytes = (input_bytes as f64 * sample_out as f64 / sample_in as f64).round() as u64;
    Some(Estimate { files: items.len(), input_bytes, output_bytes, sampled })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("quokka-imgconv-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample(path: &Path, alpha: bool) {
        if alpha {
            let img = RgbaImage::from_fn(40, 30, |x, y| Rgba([x as u8 * 6, y as u8 * 8, 128, if x < 20 { 0 } else { 255 }]));
            img.save(path).unwrap();
        } else {
            RgbImage::from_fn(40, 30, |x, y| Rgb([x as u8 * 6, y as u8 * 8, 64])).save(path).unwrap();
        }
    }

    fn run_sync(sources: Vec<PathBuf>, dest: PathBuf, o: ConvertOptions) -> Result<String, String> {
        let mut job = start_convert(sources, dest, o, Waker::noop());
        for _ in 0..3000 {
            job.poll();
            if let Some(r) = job.finished.take() {
                return r;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("conversion did not finish");
    }

    #[test]
    fn converts_to_every_format_and_reads_back() {
        let d = tmpdir("all");
        let src = d.join("pic.png");
        sample(&src, true);
        for f in ImageFormat::ALL {
            for lossless in [false, true] {
                if lossless && f != ImageFormat::Webp {
                    continue;
                }
                let out = d.join(format!("out-{}-{lossless}", f.ext()));
                let o = ConvertOptions { format: f, quality: 70, lossless, ..Default::default() };
                let r = run_sync(vec![src.clone()], out.clone(), o);
                assert!(r.is_ok(), "{f:?}: {r:?}");
                let target = out.join(format!("pic.{}", f.ext()));
                assert!(target.is_file(), "{f:?}");
                // AVIF decoding needs dav1d, which is not built in
                if f != ImageFormat::Avif {
                    let back = image::open(&target).unwrap_or_else(|e| panic!("{f:?}: {e}"));
                    assert_eq!((back.width(), back.height()), (40, 30), "{f:?}");
                }
            }
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn jpeg_gets_white_background_and_quality_changes_size() {
        let d = tmpdir("jpeg");
        let src = d.join("a.png");
        sample(&src, true);
        let size = |q: u8, dir: &str| {
            let out = d.join(dir);
            run_sync(vec![src.clone()], out.clone(), ConvertOptions { format: ImageFormat::Jpeg, quality: q, ..Default::default() }).unwrap();
            fs::metadata(out.join("a.jpg")).unwrap().len()
        };
        assert!(size(10, "low") < size(95, "high"));
        let px = image::open(d.join("high/a.jpg")).unwrap().to_rgb8().get_pixel(2, 2).0;
        assert!(px.iter().all(|&c| c > 240), "transparent area should be white: {px:?}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn folders_keep_structure_existing_skipped_broken_reported() {
        let d = tmpdir("tree");
        let root = d.join("photos");
        fs::create_dir_all(root.join("2024")).unwrap();
        sample(&root.join("a.png"), false);
        sample(&root.join("2024/b.bmp"), false);
        fs::write(root.join("notes.txt"), "x").unwrap();
        fs::write(root.join("broken.jpg"), "not a jpeg").unwrap();
        let out = d.join("out");
        fs::create_dir_all(out.join("photos")).unwrap();
        fs::write(out.join("photos/a.webp"), "keep me").unwrap();

        let r = run_sync(vec![root.clone()], out.clone(), ConvertOptions::default());
        let err = r.unwrap_err();
        assert!(err.contains("broken.jpg"), "{err}");
        assert!(out.join("photos/2024/b.webp").is_file());
        assert_eq!(fs::read_to_string(out.join("photos/a.webp")).unwrap(), "keep me");
        assert!(!out.join("photos/notes.webp").exists());
        // no temporary files left behind
        assert!(!fs::read_dir(out.join("photos")).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".quokka-tmp")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn in_place_recompress_with_overwrite() {
        let d = tmpdir("inplace");
        let src = d.join("x.jpg");
        RgbImage::from_fn(200, 200, |x, y| Rgb([(x ^ y) as u8, x as u8, y as u8])).save(&src).unwrap();
        let before = fs::metadata(&src).unwrap().len();
        let o = ConvertOptions { format: ImageFormat::Jpeg, quality: 5, overwrite: true, ..Default::default() };
        run_sync(vec![src.clone()], d.clone(), o).unwrap();
        assert!(fs::metadata(&src).unwrap().len() < before);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn estimate_scales_sample_ratio() {
        let d = tmpdir("estimate");
        for i in 0..6 {
            RgbImage::from_fn(120, 90, |x, y| Rgb([(x * y) as u8, x as u8 ^ i, y as u8])).save(d.join(format!("{i}.bmp"))).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let low = estimate(&[d.clone()], &ConvertOptions { format: ImageFormat::Jpeg, quality: 10, ..Default::default() }, &cancel).unwrap();
        let high = estimate(&[d.clone()], &ConvertOptions { format: ImageFormat::Jpeg, quality: 95, ..Default::default() }, &cancel).unwrap();
        assert_eq!((low.files, low.sampled), (6, 4));
        assert!(low.output_bytes < high.output_bytes && high.output_bytes < low.input_bytes);
        cancel.store(true, Ordering::Relaxed);
        assert!(estimate(&[d.clone()], &ConvertOptions::default(), &cancel).is_none());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn nothing_readable_is_an_error() {
        let d = tmpdir("none");
        let f = d.join("a.txt");
        fs::write(&f, "x").unwrap();
        assert!(run_sync(vec![f], d.clone(), ConvertOptions::default()).is_err());
        let _ = fs::remove_dir_all(&d);
    }
}
