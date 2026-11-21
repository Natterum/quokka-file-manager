#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// UI layer (egui). Composition root: wires the infrastructure adapters into the
// application layer; see ARCHITECTURE.md for the layers.
mod app;
mod config;
mod dnd;
mod file_icons;
mod icons;
mod logo;
mod theme;
mod tray;
mod util;

// the other layers under their familiar module names
use quokka_core::{panel, queue};
use quokka_infra::{clipboard, fsops, iconsrc, search};

use eframe::egui;

/// Write panics to %APPDATA%\\Quokka\\crash.log and tell the user, since the release
/// build has no console where the message would otherwise go.
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!(
            "[{}] {info}\n{}\n\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            std::backtrace::Backtrace::force_capture()
        );
        if let Some(dir) = std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("Quokka")) {
            let _ = std::fs::create_dir_all(&dir);
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
                let _ = f.write_all(msg.as_bytes());
            }
        }
        default(info);
    }));
}

/// Window title; also how a second launch finds the running window.
pub const TITLE: &str = "Quokka — файловый менеджер";
/// Window titles in every interface language (index = `Lang as usize`); a second
/// launch looks for the running window by any of them.
pub const TITLES: [&str; 2] = [TITLE, "Quokka — file manager"];

/// Window title in the current interface language.
pub fn title() -> &'static str {
    TITLES[quokka_domain::lang::get() as usize]
}

fn main() -> eframe::Result<()> {
    install_crash_log();
    // optional first argument: folder to open
    let start = std::env::args_os().nth(1).map(std::path::PathBuf::from).filter(|p| p.is_dir());
    // defaults (names of the standard favourites) follow the system language
    quokka_domain::lang::set(config::system_language());
    let settings = config::Settings::load();
    quokka_domain::lang::set(settings.language);
    if tray::forward_to_running(&TITLES, start.as_deref(), settings.multi_instance) {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title())
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([960.0, 600.0])
            .with_icon(std::sync::Arc::new(logo::icon_data(256))),
        ..Default::default()
    };
    eframe::run_native(
        // app id (storage folder), never translated
        TITLE,
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::QuokkaApp::new(cc, start)))
        }),
    )
}
