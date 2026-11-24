//! Update check and self-update (GitHub Releases): a quiet check once a day at
//! start-up, «Справка → Проверить обновления…», and the dialog that downloads the new
//! version, verifies it and restarts Quokka. The network work runs on background
//! threads; results arrive through [`Inbox`].

use quokka_domain::{tr, trf};
use super::*;
use quokka_domain::update::{self as rules, InstallKind};
use quokka_infra::update::{self, Release};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What the background threads report.
pub(super) enum Event {
    Checked { manual: bool, result: Result<Release, String> },
    Downloaded(Result<PathBuf, String>),
}

pub(super) type Inbox = Arc<Mutex<Vec<Event>>>;

#[derive(Default)]
pub(super) struct Progress {
    done: AtomicU64,
    total: AtomicU64,
}

pub(super) enum Phase {
    Checking,
    UpToDate,
    Failed(String),
    Available(Release),
    Downloading { release: Release, progress: Arc<Progress>, cancel: Arc<AtomicBool> },
    Installing,
}

pub(super) struct UpdateDialog {
    phase: Phase,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn releases_page() -> String {
    format!("https://github.com/{}/releases", update::REPO)
}

/// Release notes without the Markdown that would show as clutter in a label.
fn plain_notes(md: &str) -> String {
    let mut out = Vec::new();
    for line in md.lines() {
        let t = line.trim_end();
        let t = t.trim_start_matches('#').trim_start();
        let t = match t.strip_prefix("* ").or_else(|| t.strip_prefix("- ")) {
            Some(rest) => format!("•  {rest}"),
            None => t.to_string(),
        };
        out.push(t.replace("**", "").replace('`', ""));
    }
    let text = out.join("\n");
    text.trim().to_string()
}

impl QuokkaApp {
    /// Quiet check at start-up, at most once a day.
    pub(super) fn auto_check_updates(&mut self) {
        if self.settings.check_updates && rules::check_due(self.settings.update_checked_at, now()) {
            self.start_update_check(false);
        }
    }

    fn start_update_check(&self, manual: bool) {
        let (inbox, waker) = (self.update_inbox.clone(), self.services.waker.clone());
        std::thread::spawn(move || {
            let result = update::latest();
            inbox.lock().unwrap_or_else(|e| e.into_inner()).push(Event::Checked { manual, result });
            waker.wake();
        });
    }

    /// «Проверить обновления…»: show the dialog at once and fill it when the answer comes.
    pub(super) fn check_updates_now(&mut self) {
        self.modal = Modal::Update(Box::new(UpdateDialog { phase: Phase::Checking }));
        self.start_update_check(true);
    }

    fn offer(&self, r: &Release) -> bool {
        rules::is_newer(&r.tag, current())
    }

    /// Handle what the background threads reported; called every frame.
    pub(super) fn poll_updates(&mut self) {
        let events = std::mem::take(&mut *self.update_inbox.lock().unwrap_or_else(|e| e.into_inner()));
        for ev in events {
            match ev {
                Event::Checked { manual, result } => {
                    if result.is_ok() {
                        self.settings.update_checked_at = now();
                        self.save_settings();
                    }
                    if manual {
                        let Modal::Update(d) = &mut self.modal else { continue };
                        if !matches!(d.phase, Phase::Checking) {
                            continue;
                        }
                        d.phase = match result {
                            Ok(r) if rules::is_newer(&r.tag, current()) => Phase::Available(r),
                            Ok(_) => Phase::UpToDate,
                            Err(e) => Phase::Failed(e),
                        };
                    } else if let Ok(r) = result {
                        if self.offer(&r) && r.version.to_string() != self.settings.update_skip {
                            self.update_offer = Some(r);
                        }
                    }
                }
                Event::Downloaded(result) => {
                    let Modal::Update(d) = &mut self.modal else { continue };
                    let Phase::Downloading { cancel, .. } = &d.phase else { continue };
                    if cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    match result.and_then(|pkg| update::apply(update::install_kind(), &pkg)) {
                        Ok(()) => {
                            d.phase = Phase::Installing;
                            // the new version starts once this one has exited
                            self.save_session();
                            self.allow_close = true;
                            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(e) => d.phase = Phase::Failed(e),
                    }
                }
            }
        }
        // an update found in the background waits until no other dialog is open
        if matches!(self.modal, Modal::None) {
            if let Some(r) = self.update_offer.take() {
                self.modal = Modal::Update(Box::new(UpdateDialog { phase: Phase::Available(r) }));
            }
        }
    }

    fn start_download(&self, release: Release) -> Phase {
        let progress = Arc::new(Progress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let (inbox, waker) = (self.update_inbox.clone(), self.services.waker.clone());
        let (r, p, c) = (release.clone(), progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let w = waker.clone();
            let result = update::download(&r, update::install_kind(), &c, &|done, total| {
                p.done.store(done, Ordering::Relaxed);
                p.total.store(total, Ordering::Relaxed);
                w.wake();
            });
            inbox.lock().unwrap_or_else(|e| e.into_inner()).push(Event::Downloaded(result));
            waker.wake();
        });
        Phase::Downloading { release, progress, cancel }
    }

    /// Draw the update dialog; returns whether it stays open.
    pub(super) fn update_ui(&mut self, ctx: &egui::Context, d: &mut UpdateDialog, esc: bool) -> bool {
        let mut close = esc;
        let mut next = None;
        let mut open_page: Option<String> = None;
        let busy = self.queue.is_active();
        let kind = update::install_kind();
        let width = 560.0_f32.min(ctx.content_rect().width() - 64.0);
        dialog(ctx, "update", width, |ui| match &d.phase {
            Phase::Checking => {
                dialog_header(ui, Glyph::Download, BLUE, tr!("Проверка обновлений", "Checking for updates"), Some(&trf!("Установлена версия {}", "Installed version {}", current())));
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(16.0).color(BLUE));
                    ui.label(RichText::new(tr!("Связываюсь с GitHub…", "Contacting GitHub…")).color(TEXT_SOFT()));
                });
                ui.add_space(14.0);
                buttons_row(ui, |ui| {
                    if jb_btn(ui, tr!("Закрыть", "Close"), false).clicked() {
                        close = true;
                    }
                });
            }
            Phase::UpToDate => {
                dialog_header(ui, Glyph::Check, GREEN, tr!("Обновлений нет", "No updates"), Some(&trf!("У вас последняя версия — {}", "You have the latest version — {}", current())));
                buttons_row(ui, |ui| {
                    if jb_btn(ui, tr!("ОК", "OK"), true).clicked() {
                        close = true;
                    }
                    if jb_btn(ui, tr!("Все выпуски", "All releases"), false).clicked() {
                        open_page = Some(releases_page());
                    }
                });
            }
            Phase::Failed(e) => {
                dialog_header(ui, Glyph::Close, RED, tr!("Не удалось обновить", "Update failed"), None);
                ui.add(egui::Label::new(RichText::new(e.as_str()).color(TEXT())).wrap());
                ui.add_space(14.0);
                buttons_row(ui, |ui| {
                    if jb_btn(ui, tr!("Закрыть", "Close"), true).clicked() {
                        close = true;
                    }
                    if jb_btn(ui, tr!("Скачать вручную", "Download manually"), false).clicked() {
                        open_page = Some(releases_page());
                    }
                });
            }
            Phase::Available(r) => {
                dialog_header(
                    ui,
                    Glyph::Download,
                    BLUE,
                    &trf!("Доступна версия {}", "Version {} is available", r.version),
                    Some(&trf!("У вас {}", "You have {}", current())),
                );
                let notes = plain_notes(&r.notes);
                if !notes.is_empty() {
                    egui::Frame::NONE
                        .fill(INPUT_BG())
                        .stroke(Stroke::new(1.0, BORDER()))
                        .corner_radius(rad(10.0))
                        .inner_margin(Margin { left: 14, right: 6, top: 10, bottom: 10 })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical().id_salt("update-notes").max_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
                                ui.set_width(ui.available_width() - 8.0);
                                ui.add(egui::Label::new(RichText::new(notes).size(13.0).color(TEXT_SOFT())).wrap());
                            });
                        });
                    ui.add_space(10.0);
                }
                let installable = r.can_install(kind);
                let how = if !installable {
                    tr!("Для этой версии нет пакета автообновления — скачайте её со страницы выпуска.", "This release has no self-update package — download it from the release page.")
                } else if busy {
                    tr!("Дождитесь окончания операций в очереди, чтобы обновиться.", "Wait for the queued operations to finish before updating.")
                } else {
                    match kind {
                        InstallKind::Msi => tr!("Будет скачан и запущен установщик MSI; Quokka закроется и откроется уже новой версией.", "The MSI installer will be downloaded and run; Quokka will close and reopen as the new version."),
                        InstallKind::Portable => tr!("Новая версия заменит quokka.exe в папке программы; Quokka перезапустится. Вкладки сохранятся.", "The new version will replace quokka.exe in the program folder; Quokka will restart. Tabs are kept."),
                    }
                };
                ui.add(egui::Label::new(RichText::new(how).size(12.5).color(MUTED())).wrap());
                ui.add_space(14.0);
                buttons_row(ui, |ui| {
                    let install = ui.add_enabled_ui(installable && !busy, |ui| jb_btn(ui, tr!("Обновить и перезапустить", "Update and restart"), true)).inner;
                    if install.clicked() {
                        next = Some(self.start_download(r.clone()));
                    }
                    if jb_btn(ui, tr!("Позже", "Later"), false).clicked() {
                        close = true;
                    }
                    if jb_btn(ui, tr!("Пропустить версию", "Skip this version"), false).clicked() {
                        self.settings.update_skip = r.version.to_string();
                        close = true;
                    }
                    if jb_btn(ui, tr!("Что нового", "What’s new"), false).clicked() {
                        open_page = Some(if r.page.is_empty() { releases_page() } else { r.page.clone() });
                    }
                });
            }
            Phase::Downloading { release, progress, cancel } => {
                dialog_header(ui, Glyph::Download, BLUE, &trf!("Загрузка версии {}", "Downloading version {}", release.version), None);
                let (done, total) = (progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed));
                let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
                icons::bar(ui.painter(), r, frac, BLUE);
                ui.add_space(8.0);
                let text = if total > 0 {
                    trf!("{} из {}", "{} of {}", format_size(done), format_size(total))
                } else {
                    format_size(done)
                };
                ui.label(RichText::new(text).size(12.5).color(MUTED()));
                ui.add_space(14.0);
                buttons_row(ui, |ui| {
                    if jb_btn(ui, tr!("Отмена", "Cancel"), false).clicked() {
                        close = true;
                    }
                });
                if close {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            Phase::Installing => {
                dialog_header(ui, Glyph::Refresh, GREEN, tr!("Перезапуск…", "Restarting…"), Some(tr!("Quokka откроется уже новой версией", "Quokka will reopen as the new version")));
            }
        });
        if let Some(url) = open_page {
            if let Err(e) = fsops::open_external(std::path::Path::new(&url)) {
                self.toast(trf!("Не удалось открыть браузер: {e}", "Could not open the browser: {e}"), true);
            }
        }
        if let Some(p) = next {
            d.phase = p;
        }
        if close {
            if let Phase::Downloading { cancel, .. } = &d.phase {
                cancel.store(true, Ordering::Relaxed);
            }
            self.save_settings();
        }
        !close
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_lose_markdown_clutter() {
        let md = "## What's Changed\r\n* **Fix** `F5` copy by @dev in #3\n- Second\n\n";
        assert_eq!(plain_notes(&md.replace("\r\n", "\n")), "What's Changed\n•  Fix F5 copy by @dev in #3\n•  Second");
    }
}
