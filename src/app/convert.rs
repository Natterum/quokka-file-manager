//! «Конвертация изображений» (Ctrl+Shift+E): target format, quality, where the results
//! go (next to the originals with a name suffix, or into a folder)
//! for the marked images and folders, with an estimate of the resulting size that is
//! recomputed in the background whenever the settings change. The rules live in
//! `quokka_domain::imgconv`, the conversion in `quokka_infra::imgconv`.

use quokka_domain::{tr, trf};
use super::*;
use quokka_domain::format::format_size;
use quokka_domain::imgconv::{can_read, target_name, ConvertOptions, ImageFormat, PngLevel};
use quokka_infra::imgconv::{estimate, Estimate};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// Wait this long after the last change before estimating, so dragging the quality
/// slider does not start a conversion per pixel.
const ESTIMATE_DELAY: f64 = 0.35;

/// Settings that change the size of the result (overwrite and dates do not).
type SizeKey = (ImageFormat, u8, bool, PngLevel);

fn size_key(o: &ConvertOptions) -> SizeKey {
    (o.format, o.quality(), o.lossless, o.png)
}

struct EstimateRun {
    key: SizeKey,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Option<Estimate>>,
}

pub(super) struct ConvertDialog {
    sources: Vec<PathBuf>,
    dest: String,
    /// Write the results to `dest` instead of next to each original.
    to_folder: bool,
    was_to_folder: bool,
    /// Focus the folder field on the frame it appears.
    focus_dest: bool,
    opts: ConvertOptions,
    first: bool,
    /// Latest finished estimate and the settings it was made for.
    estimate: Option<(SizeKey, Option<Estimate>)>,
    running: Option<EstimateRun>,
    changed_at: f64,
    last_key: SizeKey,
}

impl Drop for ConvertDialog {
    fn drop(&mut self) {
        if let Some(r) = &self.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl ConvertDialog {
    /// Start, collect or restart the background estimate; asks for repaints while waiting.
    fn tick_estimate(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let key = size_key(&self.opts);
        if key != self.last_key {
            self.last_key = key;
            self.changed_at = now;
        }
        if let Some(run) = &self.running {
            if run.key != key {
                run.cancel.store(true, Ordering::Relaxed);
                self.running = None;
            } else if let Ok(result) = run.rx.try_recv() {
                self.estimate = Some((run.key, result));
                self.running = None;
            }
        }
        let fresh = self.estimate.as_ref().is_some_and(|(k, _)| *k == key);
        if fresh || self.running.is_some() {
            return;
        }
        let wait = ESTIMATE_DELAY - (now - self.changed_at);
        if wait > 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
            return;
        }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (sources, opts, flag, repaint) = (self.sources.clone(), self.opts.clone(), cancel.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("quokka-img-estimate".into()).spawn(move || {
            let r = estimate(&sources, &opts, &flag);
            let _ = tx.send(r);
            repaint.request_repaint();
        });
        match spawned {
            Ok(_) => self.running = Some(EstimateRun { key, cancel, rx }),
            Err(_) => self.estimate = Some((key, None)),
        }
    }

    fn estimate_text(&self) -> (String, Color32) {
        let key = size_key(&self.opts);
        match &self.estimate {
            Some((_, Some(e))) if e.files == 0 => {
                (tr!("В выбранном нет изображений", "No images in the selection").into(), ORANGE)
            }
            Some((k, Some(e))) => {
                let mut s = trf!("≈ {} вместо {}", "≈ {} instead of {}", format_size(e.output_bytes), format_size(e.input_bytes));
                if e.input_bytes > 0 {
                    let pct = (e.output_bytes as f64 / e.input_bytes as f64 - 1.0) * 100.0;
                    s.push_str(&format!(" ({}{:.0}%)", if pct >= 0.0 { "+" } else { "−" }, pct.abs()));
                }
                let stale = *k != key;
                let color = if stale { MUTED } else if e.output_bytes <= e.input_bytes { GREEN } else { ORANGE };
                (s, color)
            }
            Some((k, None)) if *k == key => (tr!("Не удалось оценить размер", "Could not estimate the size").into(), MUTED),
            _ => (tr!("Оцениваю размер…", "Estimating size…").into(), MUTED),
        }
    }

    fn estimate_note(&self) -> Option<String> {
        let (_, Some(e)) = self.estimate.as_ref()? else { return None };
        if e.files == 0 {
            return None;
        }
        Some(if e.sampled < e.files {
            trf!("{} · оценка по {} из них", "{} · estimated from {} of them", items_text(e.files as u64), e.sampled)
        } else {
            items_text(e.files as u64)
        })
    }
}

fn section(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).size(13.0).color(TEXT_SOFT));
}

/// A format tile: name on top, short hint below, highlighted when selected.
fn format_tile(ui: &mut Ui, f: ImageFormat, selected: bool, width: f32) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(width, 34.0), Sense::click());
    let (fill, stroke) = if selected {
        (BLUE_SOFT, BLUE)
    } else if resp.hovered() {
        (HOVER, mix(BORDER, BLUE, 0.4))
    } else {
        (INPUT_BG, BORDER)
    };
    let p = ui.painter();
    p.rect(r, 8.0, fill, Stroke::new(1.0, stroke));
    let g = p.layout_no_wrap(f.label().to_string(), semibold(13.5), if selected { BLUE } else { TEXT });
    p.galley(r.center() - g.size() / 2.0, g, TEXT);
    resp.on_hover_text(f.hint()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl QuokkaApp {
    pub(super) fn open_convert(&mut self, ctx: &egui::Context) {
        let sources: Vec<PathBuf> = self.ap_ref().targets().into_iter().filter(|p| p.is_dir() || can_read(p)).collect();
        if sources.is_empty() {
            self.toast(tr!("Выберите изображения или папки с ними", "Select images or folders with images"), true);
            return;
        }
        let opts = ctx.data(|d| d.get_temp::<ConvertOptions>(Id::new("convert-opts"))).unwrap_or_default();
        // by default the results go next to the originals; «В папку» offers the other panel, like F5
        let to_folder = ctx.data(|d| d.get_temp::<bool>(Id::new("convert-to-folder"))).unwrap_or(false);
        let dest = self.dest_dir().display().to_string();
        let key = size_key(&opts);
        self.modal = Modal::Convert(Box::new(ConvertDialog {
            sources,
            dest,
            to_folder,
            was_to_folder: to_folder,
            focus_dest: false,
            opts,
            first: true,
            estimate: None,
            running: None,
            changed_at: f64::NEG_INFINITY,
            last_key: key,
        }));
    }

    /// Returns whether the dialog stays open.
    pub(super) fn convert_ui(&mut self, ctx: &egui::Context, d: &mut ConvertDialog, esc: bool, enter: bool) -> bool {
        if esc {
            return false;
        }
        d.tick_estimate(ctx);
        let mut submit = false;
        let mut cancel = false;
        let sub = match d.sources.as_slice() {
            [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            many => items_text(many.len() as u64),
        };
        dialog(ctx, "convert-images", 560.0, |ui| {
            dialog_header(ui, Glyph::Image, PURPLE, tr!("Конвертация изображений", "Convert images"), Some(&sub));
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);

            // ── format
            section(ui, tr!("Формат", "Format"));
            let w = ui.available_width();
            let cols = 5;
            let tile_w = (w - 8.0 * (cols as f32 - 1.0)) / cols as f32;
            for row in ImageFormat::ALL.chunks(cols) {
                ui.horizontal(|ui| {
                    for &f in row {
                        if format_tile(ui, f, d.opts.format == f, tile_w).clicked() {
                            d.opts.format = f;
                        }
                    }
                });
            }
            ui.label(RichText::new(d.opts.format.hint()).size(12.5).color(MUTED));

            // ── quality
            let f = d.opts.format;
            if f == ImageFormat::Webp {
                ui.checkbox(&mut d.opts.lossless, tr!("Без потерь", "Lossless"));
            }
            if f.has_quality() && !(f == ImageFormat::Webp && d.opts.lossless) {
                section(ui, tr!("Качество", "Quality"));
                ui.horizontal(|ui| {
                    ui.spacing_mut().slider_width = (w - 140.0).max(120.0);
                    let mut q = d.opts.quality();
                    ui.add(egui::Slider::new(&mut q, 1..=100).suffix("%"));
                    d.opts.quality = q;
                });
                let tip = match d.opts.quality() {
                    90..=100 => tr!("Почти без видимых потерь, крупные файлы", "Nearly lossless, larger files"),
                    75..=89 => tr!("Хороший баланс для фото", "Good balance for photos"),
                    50..=74 => tr!("Заметно меньше, мелкие артефакты", "Much smaller, minor artifacts"),
                    _ => tr!("Очень маленькие файлы, заметные артефакты", "Tiny files, visible artifacts"),
                };
                ui.label(RichText::new(tip).size(12.5).color(MUTED));
            } else if f == ImageFormat::Png {
                section(ui, tr!("Сжатие (без потерь)", "Compression (lossless)"));
                ui.horizontal(|ui| {
                    for level in PngLevel::ALL {
                        ui.radio_value(&mut d.opts.png, level, level.label());
                    }
                });
            }
            if f == ImageFormat::Jpeg {
                ui.label(RichText::new(tr!("Прозрачные области станут белыми", "Transparent areas become white")).size(12.5).color(MUTED));
            }

            // ── size estimate
            ui.add_space(2.0);
            let (text, color) = d.estimate_text();
            egui::Frame::none().fill(INPUT_BG).rounding(8.0).inner_margin(vec2(12.0, 9.0)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr!("Размер после", "Size after")).size(13.0).color(TEXT_SOFT));
                    ui.label(RichText::new(text).size(14.0).strong().color(color));
                    if d.running.is_some() {
                        ui.add(egui::Spinner::new().size(14.0));
                    }
                });
                if let Some(note) = d.estimate_note() {
                    ui.label(RichText::new(note).size(12.0).color(MUTED));
                }
            });

            // ── where the results go
            section(ui, tr!("Куда сохранять", "Save to"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.to_folder, false, tr!("Рядом с исходными", "Next to the originals"));
                ui.radio_value(&mut d.to_folder, true, tr!("В папку", "To folder"));
            });
            if d.to_folder {
                let r = ui.add(egui::TextEdit::singleline(&mut d.dest).desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)).font(regular(15.0)));
                if std::mem::take(&mut d.focus_dest) {
                    r.request_focus();
                }
                if r.lost_focus() && enter {
                    submit = true;
                }
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr!("Добавить к имени", "Add to name")).size(13.0).color(TEXT_SOFT));
                let r = ui.add(egui::TextEdit::singleline(&mut d.opts.suffix).desired_width(140.0).margin(vec2(8.0, 6.0)).font(mono(14.0)));
                if d.first {
                    d.first = false;
                    r.request_focus();
                }
                if r.lost_focus() && enter {
                    submit = true;
                }
                let example = d.sources.iter().find(|p| p.is_file()).cloned().unwrap_or_else(|| PathBuf::from(tr!("фото.jpg", "photo.jpg")));
                let name = target_name(&example, d.opts.format, &d.opts.suffix);
                let from = example.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                ui.label(RichText::new(format!("{from} → {}", name.to_string_lossy())).size(12.5).color(MUTED));
            });
            if !d.to_folder && d.opts.suffix.is_empty() {
                let warn = if d.opts.overwrite {
                    tr!("Без суффикса файлы того же формата заменят исходные", "Without a suffix, files of the same format replace the originals")
                } else {
                    tr!("Без суффикса файлы того же формата будут пропущены", "Without a suffix, files of the same format are skipped")
                };
                ui.label(RichText::new(warn).size(12.5).color(ORANGE));
            }
            ui.checkbox(&mut d.opts.overwrite, tr!("Заменять существующие файлы", "Overwrite existing files"));
            ui.checkbox(&mut d.opts.keep_time, tr!("Сохранять дату изменения", "Keep date modified"));
            ui.add_space(10.0);
            buttons_row(ui, |ui| {
                if btn(ui, tr!("Конвертировать", "Convert"), PURPLE, Color32::WHITE).clicked() {
                    submit = true;
                }
                if btn(ui, tr!("Отмена", "Cancel"), INPUT_BG, TEXT).clicked() {
                    cancel = true;
                }
            });
        });
        if d.to_folder != d.was_to_folder {
            d.was_to_folder = d.to_folder;
            d.focus_dest = d.to_folder;
        }
        ctx.data_mut(|m| {
            m.insert_temp(Id::new("convert-opts"), d.opts.clone());
            m.insert_temp(Id::new("convert-to-folder"), d.to_folder);
        });
        if cancel {
            return false;
        }
        if !submit {
            return true;
        }
        if d.opts.suffix.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
            self.toast(tr!("Недопустимый символ в добавке к имени", "Invalid character in the name suffix"), true);
            return true;
        }
        if !d.to_folder {
            self.enqueue(OpSpec::ConvertImages { sources: std::mem::take(&mut d.sources), dest: None, options: d.opts.clone() });
            return false;
        }
        let mut dest = PathBuf::from(d.dest.trim());
        if dest.as_os_str().is_empty() {
            self.toast(tr!("Укажите папку назначения", "Specify the destination folder"), true);
            return true;
        }
        if dest.is_relative() {
            dest = self.ap_ref().path.join(dest);
        }
        if dest.exists() && !dest.is_dir() {
            self.toast(tr!("Назначение должно быть папкой", "The destination must be a folder"), true);
            return true;
        }
        self.enqueue(OpSpec::ConvertImages { sources: std::mem::take(&mut d.sources), dest: Some(dest), options: d.opts.clone() });
        false
    }
}
