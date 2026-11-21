//! Colours, fonts and egui style, modelled after the reference dashboard.

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, Rounding, Stroke, TextStyle};

pub const BG: Color32 = Color32::from_rgb(244, 247, 251);
pub const CARD: Color32 = Color32::WHITE;
pub const BORDER: Color32 = Color32::from_rgb(232, 236, 243);
pub const TEXT: Color32 = Color32::from_rgb(33, 39, 55);
pub const TEXT_SOFT: Color32 = Color32::from_rgb(88, 97, 116);
pub const MUTED: Color32 = Color32::from_rgb(150, 158, 174);
pub const HOVER: Color32 = Color32::from_rgb(246, 248, 252);
pub const TRACK: Color32 = Color32::from_rgb(234, 238, 244);
pub const INPUT_BG: Color32 = Color32::from_rgb(245, 247, 251);

pub const BLUE: Color32 = Color32::from_rgb(22, 119, 255);
pub const BLUE_SOFT: Color32 = Color32::from_rgb(234, 242, 255);
pub const YELLOW: Color32 = Color32::from_rgb(255, 196, 61);
pub const RED: Color32 = Color32::from_rgb(240, 90, 90);
pub const TEAL: Color32 = Color32::from_rgb(38, 190, 160);
pub const PURPLE: Color32 = Color32::from_rgb(139, 108, 240);
pub const ORANGE: Color32 = Color32::from_rgb(255, 150, 70);
pub const GREEN: Color32 = Color32::from_rgb(52, 186, 120);
pub const SLATE: Color32 = Color32::from_rgb(100, 116, 139);
pub const GRAY: Color32 = Color32::from_rgb(160, 168, 184);

/// Default button padding, the same as «Готово» in the settings (see `sized_btn`),
/// so plain egui buttons, combo boxes and toggles match the dialog buttons.
pub const BUTTON_PADDING_X: f32 = 16.0;
pub const BUTTON_PADDING_Y: f32 = 4.0;

pub const FOLDER_BACK: Color32 = Color32::from_rgb(247, 182, 58);
pub const FOLDER_FRONT: Color32 = Color32::from_rgb(255, 210, 106);
pub const FOLDER_LIGHT: Color32 = Color32::from_rgb(255, 229, 160);

const SEMIBOLD: &str = "semibold";

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

fn load_first(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    // Primary UI font: Montserrat, embedded so it works without being installed.
    fonts.font_data.insert(
        "montserrat".into(),
        FontData::from_static(include_bytes!("../assets/fonts/Montserrat-Regular.ttf")),
    );
    fonts.font_data.insert(
        "montserrat-semibold".into(),
        FontData::from_static(include_bytes!("../assets/fonts/Montserrat-SemiBold.ttf")),
    );

    // System fonts stay as fallbacks for glyphs Montserrat lacks.
    let regular = load_first(&[
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]);
    let bold = load_first(&[
        "C:\\Windows\\Fonts\\seguisb.ttf",
        "C:\\Windows\\Fonts\\segoeuib.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    ]);
    let monospace = load_first(&["C:\\Windows\\Fonts\\consola.ttf", "C:\\Windows\\Fonts\\cascadiamono.ttf"]);

    let mut sb_family = fonts.families[&FontFamily::Proportional].clone();
    {
        let prop = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
        if let Some(data) = regular {
            fonts.font_data.insert("ui".into(), FontData::from_owned(data));
            prop.insert(0, "ui".into());
        }
        prop.insert(0, "montserrat".into());
    }
    if let Some(data) = bold {
        fonts.font_data.insert("ui-semibold".into(), FontData::from_owned(data));
        sb_family.insert(0, "ui-semibold".into());
    }
    sb_family.insert(0, "montserrat-semibold".into());
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), sb_family);
    if let Some(data) = monospace {
        fonts.font_data.insert("ui-mono".into(), FontData::from_owned(data));
        fonts.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, "ui-mono".into());
    }
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    setup_fonts(ctx);

    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::light();
    let v = &mut style.visuals;
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_rounding = Rounding::same(16.0);
    v.menu_rounding = Rounding::same(14.0);
    v.extreme_bg_color = INPUT_BG;
    v.faint_bg_color = HOVER;
    v.code_bg_color = INPUT_BG;
    v.hyperlink_color = BLUE;
    v.selection.bg_fill = Color32::from_rgb(198, 222, 255);
    v.selection.stroke = Stroke::new(1.0, BLUE);
    v.window_shadow = egui::Shadow {
        offset: egui::vec2(0.0, 10.0),
        blur: 30.0,
        spread: 0.0,
        color: Color32::from_black_alpha(28),
    };
    v.popup_shadow = egui::Shadow {
        offset: egui::vec2(0.0, 10.0),
        blur: 28.0,
        spread: 0.0,
        color: Color32::from_black_alpha(30),
    };

    let w = &mut v.widgets;
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.rounding = Rounding::same(8.0);
        wv.fg_stroke = Stroke::new(1.0, TEXT);
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.bg_fill = Color32::from_rgb(241, 244, 249);
    w.inactive.weak_bg_fill = Color32::from_rgb(241, 244, 249);
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.bg_fill = Color32::from_rgb(229, 236, 247);
    w.hovered.weak_bg_fill = Color32::from_rgb(229, 236, 247);
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(208, 222, 244));
    w.active.bg_fill = Color32::from_rgb(214, 228, 250);
    w.active.weak_bg_fill = Color32::from_rgb(214, 228, 250);
    w.active.bg_stroke = Stroke::new(1.0, BLUE);

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(BUTTON_PADDING_X, BUTTON_PADDING_Y);
    style.spacing.interact_size.y = 28.0;
    style.spacing.menu_margin = egui::Margin::same(8.0);
    style.text_styles = [
        (TextStyle::Small, regular(11.5)),
        (TextStyle::Body, regular(14.0)),
        (TextStyle::Button, regular(14.0)),
        (TextStyle::Heading, semibold(20.0)),
        (TextStyle::Monospace, mono(13.0)),
    ]
    .into();
    // The system theme may be selected on the first frame, after setup.
    // Keep Quokka's appearance and spacing in both theme slots.
    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.options_mut(apply_click_tolerance);
}

/// egui's click defaults (0.3 s between clicks, 6 pt of travel) are stricter than
/// Windows, so a double click with a slight hand tremor or a relaxed pace is lost.
/// Use the system double-click time and allow a little more travel.
fn apply_click_tolerance(options: &mut egui::Options) {
    let input = &mut options.input_options;
    input.max_click_dist = input.max_click_dist.max(MAX_CLICK_DIST);
    input.max_double_click_delay = system_double_click_secs().unwrap_or(0.5);
}

const MAX_CLICK_DIST: f32 = 10.0;

#[cfg(windows)]
fn system_double_click_secs() -> Option<f64> {
    // SAFETY: no arguments, reads a per-user system setting.
    let ms = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
    (ms > 0).then(|| (ms as f64 / 1000.0).clamp(0.2, 1.5))
}

#[cfg(not(windows))]
fn system_double_click_secs() -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_padding_survives_system_theme_selection() {
        for system_theme in [egui::Theme::Light, egui::Theme::Dark] {
            let ctx = egui::Context::default();
            apply(&ctx);
            let input = egui::RawInput {
                system_theme: Some(system_theme),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    assert_eq!(ui.spacing().button_padding, egui::vec2(BUTTON_PADDING_X, BUTTON_PADDING_Y));
                    let text = "Готово";
                    let text_size = ui.painter().layout_no_wrap(text.into(), semibold(14.0), TEXT).size();
                    let response = ui.add(egui::Button::new(egui::RichText::new(text).font(semibold(14.0))));
                    assert!(response.rect.width() >= text_size.x + BUTTON_PADDING_X * 2.0);
                    assert!(response.rect.height() >= text_size.y + BUTTON_PADDING_Y * 2.0);
                });
            });
        }
    }
}
