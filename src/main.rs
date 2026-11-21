#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod clipboard;
mod config;
mod dnd;
mod fsops;
mod folder_size;
mod icons;
mod iconsrc;
mod file_icons;
mod logo;
mod panel;
mod queue;
mod search;
mod theme;
mod util;
mod zip;

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

fn main() -> eframe::Result<()> {
    install_crash_log();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Quokka — файловый менеджер")
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([960.0, 600.0])
            .with_icon(std::sync::Arc::new(logo::icon_data(256))),
        ..Default::default()
    };
    eframe::run_native(
        "Quokka — файловый менеджер",
        options,
        Box::new(|cc| {
            // optional first argument: folder to open in the left pane
            let start = std::env::args_os().nth(1).map(std::path::PathBuf::from).filter(|p| p.is_dir());
            Ok(Box::new(app::QuokkaApp::new(cc, start)))
        }),
    )
}
