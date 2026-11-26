//! Colours, fonts and egui style, modelled after the reference dashboard.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, CornerRadius, Stroke, TextStyle};

/// Interface colour scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    Light,
    Dark,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 2] = [ThemeMode::Light, ThemeMode::Dark];

    pub fn key(self) -> &'static str {
        match self {
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        }
    }

    pub fn from_key(k: &str) -> Option<ThemeMode> {
        Self::ALL.into_iter().find(|t| t.key() == k)
    }
}

/// Colour preset of a theme: the accent colour and a tint of the neutral surfaces.
/// The light and the dark theme each have their own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorPreset {
    /// The original blue look.
    #[default]
    Classic,
    Graphite,
    Ocean,
    Forest,
    Sunset,
    Lavender,
    Rose,
    /// Modern IDE look: neutral greys with a clear blue accent.
    Studio,
    /// Classic IDE dark: soft lifted grey, muted blue.
    Charcoal,
    /// Code-hosting dark: deep blue-black, bright blue accent.
    Midnight,
    /// The same, dimmed: slate background, softer blue.
    Dusk,
    /// AI-assistant look: near-black and cool white-blue surfaces, soft blue accent.
    Glow,
    /// Arctic blue-grey with frosty accents.
    Nordic,
    /// Warm beige light and deep teal dark.
    Solar,
    /// Warm dark grey with a lime accent.
    Ember,
    /// Pure black and white with a strong accent.
    Contrast,
}

/// What a preset changes: the accent colour in the light and the dark theme, and
/// the hue the neutral surfaces lean to (`None` — the designed colours as they are).
struct PresetSpec {
    accent_light: (u8, u8, u8),
    accent_dark: (u8, u8, u8),
    tint: Option<(i16, i16, i16)>,
    /// Brightness added to the dark theme's surfaces (negative — deeper).
    dark_lift: f32,
    /// Brightness added to the light theme's surfaces.
    light_lift: f32,
}

impl ColorPreset {
    pub const ALL: [ColorPreset; 16] = [
        ColorPreset::Classic,
        ColorPreset::Graphite,
        ColorPreset::Ocean,
        ColorPreset::Forest,
        ColorPreset::Sunset,
        ColorPreset::Lavender,
        ColorPreset::Rose,
        ColorPreset::Studio,
        ColorPreset::Charcoal,
        ColorPreset::Midnight,
        ColorPreset::Dusk,
        ColorPreset::Glow,
        ColorPreset::Nordic,
        ColorPreset::Solar,
        ColorPreset::Ember,
        ColorPreset::Contrast,
    ];

    pub fn key(self) -> &'static str {
        match self {
            ColorPreset::Classic => "classic",
            ColorPreset::Graphite => "graphite",
            ColorPreset::Ocean => "ocean",
            ColorPreset::Forest => "forest",
            ColorPreset::Sunset => "sunset",
            ColorPreset::Lavender => "lavender",
            ColorPreset::Rose => "rose",
            ColorPreset::Studio => "studio",
            ColorPreset::Charcoal => "charcoal",
            ColorPreset::Midnight => "midnight",
            ColorPreset::Dusk => "dusk",
            ColorPreset::Glow => "glow",
            ColorPreset::Nordic => "nordic",
            ColorPreset::Solar => "solar",
            ColorPreset::Ember => "ember",
            ColorPreset::Contrast => "contrast",
        }
    }

    pub fn from_key(k: &str) -> Option<ColorPreset> {
        Self::ALL.into_iter().find(|p| p.key() == k)
    }

    pub fn name(self) -> &'static str {
        use quokka_domain::tr;
        match self {
            ColorPreset::Classic => tr!("Классика", "Classic"),
            ColorPreset::Graphite => tr!("Графит", "Graphite"),
            ColorPreset::Ocean => tr!("Океан", "Ocean"),
            ColorPreset::Forest => tr!("Лес", "Forest"),
            ColorPreset::Sunset => tr!("Закат", "Sunset"),
            ColorPreset::Lavender => tr!("Лаванда", "Lavender"),
            ColorPreset::Rose => tr!("Роза", "Rose"),
            ColorPreset::Studio => tr!("Студия", "Studio"),
            ColorPreset::Charcoal => tr!("Уголь", "Charcoal"),
            ColorPreset::Midnight => tr!("Полночь", "Midnight"),
            ColorPreset::Dusk => tr!("Сумерки", "Dusk"),
            ColorPreset::Glow => tr!("Сияние", "Glow"),
            ColorPreset::Nordic => tr!("Север", "Nordic"),
            ColorPreset::Solar => tr!("Солнечная", "Solar"),
            ColorPreset::Ember => tr!("Угли", "Ember"),
            ColorPreset::Contrast => tr!("Контраст", "Contrast"),
        }
    }

    fn spec(self) -> PresetSpec {
        // accent light, accent dark, tint, dark lift, light lift
        let (accent_light, accent_dark, tint, dark_lift, light_lift) = match self {
            ColorPreset::Classic => ((22, 119, 255), (22, 119, 255), None, 0.0, 0.0),
            ColorPreset::Graphite => ((71, 85, 105), (148, 163, 184), Some((0, 0, 0)), 0.0, 0.0),
            ColorPreset::Ocean => ((8, 145, 178), (34, 184, 207), Some((-6, 3, 6)), 0.0, 0.0),
            ColorPreset::Forest => ((22, 140, 80), (60, 190, 120), Some((-4, 5, -2)), 0.0, 0.0),
            ColorPreset::Sunset => ((232, 106, 26), (255, 140, 66), Some((6, 1, -6)), 0.0, 0.0),
            ColorPreset::Lavender => ((124, 82, 230), (160, 130, 250), Some((3, -3, 7)), 0.0, 0.0),
            ColorPreset::Rose => ((219, 39, 119), (244, 104, 160), Some((7, -3, 1)), 0.0, 0.0),
            ColorPreset::Studio => ((53, 116, 240), (84, 138, 247), Some((-1, 0, 2)), 2.0, 1.0),
            ColorPreset::Charcoal => ((40, 100, 180), (88, 150, 214), Some((0, 0, 1)), 16.0, -2.0),
            ColorPreset::Midnight => ((9, 105, 218), (56, 139, 253), Some((-3, 0, 5)), -11.0, 2.0),
            ColorPreset::Dusk => ((9, 105, 218), (83, 155, 245), Some((-3, 0, 5)), 9.0, 0.0),
            ColorPreset::Glow => ((11, 87, 208), (138, 180, 248), Some((-3, 0, 4)), -10.0, 1.0),
            ColorPreset::Nordic => ((76, 110, 160), (136, 192, 208), Some((-4, 0, 6)), 20.0, -1.0),
            ColorPreset::Solar => ((38, 139, 210), (42, 161, 152), Some((8, 4, -10)), -6.0, -3.0),
            ColorPreset::Ember => ((96, 140, 20), (166, 226, 46), Some((4, 2, -4)), 14.0, -1.0),
            ColorPreset::Contrast => ((0, 82, 204), (77, 163, 255), Some((0, 0, 0)), -22.0, 8.0),
        };
        PresetSpec { accent_light, accent_dark, tint, dark_lift, light_lift }
    }

    /// Background of this preset in the light or the dark theme (for previews).
    pub fn background(self, dark: bool) -> Color32 {
        tinted(if dark { (24, 26, 31) } else { (244, 247, 251) }, self, dark)
    }

    /// Accent colour of this preset in the light or the dark theme.
    pub fn accent(self, dark: bool) -> Color32 {
        let s = self.spec();
        let (r, g, b) = if dark { s.accent_dark } else { s.accent_light };
        Color32::from_rgb(r, g, b)
    }
}

/// Everything the «Оформление» settings page controls.
#[derive(Clone, Debug, PartialEq)]
pub struct Appearance {
    /// Key from [`FONTS`].
    pub font: String,
    /// Corner rounding in percent of the designed radii: 100 — as designed, 0 — square.
    pub rounding: u8,
    pub theme: ThemeMode,
    /// Colour preset of the light theme.
    pub light_preset: ColorPreset,
    /// Colour preset of the dark theme.
    pub dark_preset: ColorPreset,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            font: DEFAULT_FONT.into(),
            rounding: 100,
            theme: ThemeMode::Light,
            light_preset: ColorPreset::Classic,
            dark_preset: ColorPreset::Classic,
        }
    }
}

impl Appearance {
    /// Preset of the theme in use.
    pub fn preset(&self) -> ColorPreset {
        match self.theme {
            ThemeMode::Light => self.light_preset,
            ThemeMode::Dark => self.dark_preset,
        }
    }
}

static DARK: AtomicBool = AtomicBool::new(false);
/// Index of the current [`ColorPreset`] in [`ColorPreset::ALL`].
static PRESET: AtomicU8 = AtomicU8::new(0);
/// `f32` bits of the rounding factor (0.0 ..= 1.0).
static ROUNDING: AtomicU32 = AtomicU32::new(0x3f80_0000);

pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

fn preset() -> ColorPreset {
    ColorPreset::ALL.get(PRESET.load(Ordering::Relaxed) as usize).copied().unwrap_or_default()
}

/// Accent colour of the current theme and preset: primary buttons, links, focus
/// and selection outlines. Named like a constant because it used to be one.
#[allow(non_snake_case)]
#[inline]
pub fn BLUE() -> Color32 {
    preset().accent(is_dark())
}

/// A designed neutral colour, re-tinted for the current preset: its brightness is
/// kept and the hue is replaced by the preset's tint. Classic keeps it as designed.
fn surface(rgb: (u8, u8, u8)) -> Color32 {
    tinted(rgb, preset(), is_dark())
}

/// [`surface`] for any preset and theme, without switching the current one.
fn tinted(rgb: (u8, u8, u8), preset: ColorPreset, dark: bool) -> Color32 {
    let (r, g, b) = rgb;
    let spec = preset.spec();
    let Some((tr, tg, tb)) = spec.tint else { return Color32::from_rgb(r, g, b) };
    let lift = if dark { spec.dark_lift } else { spec.light_lift };
    let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b) + lift;
    // dark surfaces take a stronger tint, so it stays visible
    let k = if dark { 1.6 } else { 1.0 };
    let c = |t: i16| (luma + f32::from(t) * k).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(c(tr), c(tg), c(tb))
}

/// A soft fill made from the accent: the designed colour for Classic, otherwise
/// the accent laid over the card colour with the given strength (light, dark).
fn accent_soft(rgb: (u8, u8, u8), light_t: f32, dark_t: f32) -> Color32 {
    let (r, g, b) = rgb;
    if preset().spec().tint.is_none() {
        return Color32::from_rgb(r, g, b);
    }
    let dark = is_dark();
    let base = surface(if dark { (34, 37, 43) } else { (255, 255, 255) });
    crate::util::mix(base, BLUE(), if dark { dark_t } else { light_t })
}

/// Scale a designed corner radius by the user's rounding setting.
pub fn rad(r: f32) -> f32 {
    r * f32::from_bits(ROUNDING.load(Ordering::Relaxed))
}

/// [`rad`] for the `u8` radii of [`CornerRadius`].
pub fn rad_u8(r: u8) -> u8 {
    rad(f32::from(r)).round() as u8
}

/// Neutral colours that differ between the light and the dark theme.
/// Named like constants because they used to be ones; each reads the current theme.
macro_rules! palette {
    ($($(#[$m:meta])* $name:ident = $light:expr, $dark:expr;)*) => {$(
        $(#[$m])*
        #[allow(non_snake_case)]
        #[inline]
        pub fn $name() -> Color32 {
            let (r, g, b) = if is_dark() { $dark } else { $light };
            Color32::from_rgb(r, g, b)
        }
    )*};
}

/// Neutral surfaces: re-tinted by the colour preset.
macro_rules! surfaces {
    ($($(#[$m:meta])* $name:ident = $light:expr, $dark:expr;)*) => {$(
        $(#[$m])*
        #[allow(non_snake_case)]
        #[inline]
        pub fn $name() -> Color32 {
            surface(if is_dark() { $dark } else { $light })
        }
    )*};
}

/// Soft accent fills, made from the preset's accent with the given strength (light, dark).
macro_rules! accent_fills {
    ($($(#[$m:meta])* $name:ident = $light:expr, $dark:expr, $lt:expr, $dt:expr;)*) => {$(
        $(#[$m])*
        #[allow(non_snake_case)]
        #[inline]
        pub fn $name() -> Color32 {
            accent_soft(if is_dark() { $dark } else { $light }, $lt, $dt)
        }
    )*};
}

surfaces! {
    BG = (244, 247, 251), (24, 26, 31);
    CARD = (255, 255, 255), (34, 37, 43);
    BORDER = (232, 236, 243), (52, 56, 65);
    HOVER = (246, 248, 252), (44, 48, 56);
    TRACK = (234, 238, 244), (50, 54, 63);
    INPUT_BG = (245, 247, 251), (28, 30, 36);
    SEL_INACTIVE = (236, 240, 246), (48, 52, 61);
    HEADER_BG = (243, 247, 252), (30, 33, 39);
    ROW_LINE = (238, 241, 246), (42, 45, 52);
    /// Background of side trees and footers in flat dialogs.
    NAV_BG = (247, 248, 250), (29, 31, 37);
    FIELD_BORDER = (201, 205, 214), (68, 74, 86);
}

accent_fills! {
    BLUE_SOFT = (234, 242, 255), (30, 50, 84), 0.09, 0.22;
    SEL_ACTIVE = (214, 231, 255), (36, 64, 108), 0.17, 0.36;
    TAB_ACTIVE = (219, 234, 254), (36, 58, 98), 0.15, 0.32;
    NAV_SEL = (212, 226, 255), (36, 60, 104), 0.18, 0.34;
    /// Light blue fill of pressed and highlighted buttons.
    PRESSED = (218, 232, 255), (40, 66, 110), 0.16, 0.36;
}

palette! {
    TEXT = (33, 39, 55), (226, 230, 238);
    TEXT_SOFT = (88, 97, 116), (178, 185, 198);
    MUTED = (150, 158, 174), (126, 134, 150);
    SLATE = (100, 116, 139), (150, 164, 184);
    /// Glyphs and keys of disabled controls.
    DISABLED = (200, 205, 214), (86, 92, 104);
    MARKED_BG = (255, 244, 214), (70, 56, 26);
    MARKED_FG = (196, 118, 0), (240, 170, 60);
    SEARCH_HIT = (255, 232, 140), (94, 80, 28);
    /// Soft red behind dangerous actions and errors.
    DANGER_SOFT = (254, 236, 236), (78, 36, 38);
    DANGER_PRESSED = (252, 222, 222), (98, 42, 44);
    /// Soft amber behind warnings and conflicts.
    WARN_BG = (255, 248, 230), (64, 52, 28);
    WARN_BORDER = (240, 200, 120), (130, 100, 40);
    /// Toast notifications: dark on the light theme, a raised grey on the dark one.
    TOAST_BG = (33, 39, 55), (58, 63, 75);
    TOAST_BORDER = (33, 39, 55), (78, 85, 100);
    TOAST_FG = (255, 255, 255), (236, 239, 245);
}

/// Overlay laid over a button on hover (`pressed`: while held down): it darkens
/// on the light theme and lightens on the dark one, so it works on any fill.
pub fn hover_tint(pressed: bool) -> Color32 {
    match (is_dark(), pressed) {
        (false, false) => Color32::from_black_alpha(14),
        (false, true) => Color32::from_black_alpha(30),
        (true, false) => Color32::from_white_alpha(18),
        (true, true) => Color32::from_white_alpha(34),
    }
}

pub const YELLOW: Color32 = Color32::from_rgb(255, 196, 61);
pub const RED: Color32 = Color32::from_rgb(240, 90, 90);
pub const TEAL: Color32 = Color32::from_rgb(38, 190, 160);
pub const PURPLE: Color32 = Color32::from_rgb(139, 108, 240);
pub const ORANGE: Color32 = Color32::from_rgb(255, 150, 70);
pub const GREEN: Color32 = Color32::from_rgb(52, 186, 120);
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

/// Key of the embedded default font.
pub const DEFAULT_FONT: &str = "montserrat";

/// Fonts offered in the settings: key, name, regular and bold files in the Windows
/// font folder. Montserrat is embedded and always available.
pub const FONTS: [(&str, &str, &str, &str); 9] = [
    (DEFAULT_FONT, "Montserrat", "", ""),
    ("segoe", "Segoe UI", "segoeui.ttf", "seguisb.ttf"),
    ("arial", "Arial", "arial.ttf", "arialbd.ttf"),
    ("calibri", "Calibri", "calibri.ttf", "calibrib.ttf"),
    ("verdana", "Verdana", "verdana.ttf", "verdanab.ttf"),
    ("tahoma", "Tahoma", "tahoma.ttf", "tahomabd.ttf"),
    ("trebuchet", "Trebuchet MS", "trebuc.ttf", "trebucbd.ttf"),
    ("georgia", "Georgia", "georgia.ttf", "georgiab.ttf"),
    ("candara", "Candara", "candara.ttf", "candarab.ttf"),
];

fn system_font_path(file: &str) -> std::path::PathBuf {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    std::path::Path::new(&windir).join("Fonts").join(file)
}

/// Fonts from [`FONTS`] present on this system: `(key, name)`.
pub fn available_fonts() -> Vec<(&'static str, &'static str)> {
    FONTS
        .iter()
        .filter(|(key, _, regular, _)| *key == DEFAULT_FONT || system_font_path(regular).is_file())
        .map(|(key, name, _, _)| (*key, *name))
        .collect()
}

/// Display name of a font key; unknown keys fall back to Montserrat.
pub fn font_name(key: &str) -> &'static str {
    FONTS.iter().find(|f| f.0 == key).map_or(FONTS[0].1, |f| f.1)
}

fn load_first(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

/// Regular and bold data of a system font from [`FONTS`]; `None` for Montserrat or a missing file.
fn load_system_font(key: &str) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    let (_, _, regular, bold) = FONTS.iter().find(|f| f.0 == key && f.0 != DEFAULT_FONT)?;
    let regular = std::fs::read(system_font_path(regular)).ok()?;
    Some((regular, std::fs::read(system_font_path(bold)).ok()))
}

fn setup_fonts(ctx: &egui::Context, font: &str) {
    let mut fonts = FontDefinitions::default();

    // Default UI font: Montserrat, embedded so it works without being installed.
    fonts.font_data.insert(
        "montserrat".into(),
        std::sync::Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Montserrat-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "montserrat-semibold".into(),
        std::sync::Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Montserrat-SemiBold.ttf"))),
    );

    // System fonts stay as fallbacks for glyphs the main font lacks.
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
            fonts.font_data.insert("ui".into(), std::sync::Arc::new(FontData::from_owned(data)));
            prop.insert(0, "ui".into());
        }
        prop.insert(0, "montserrat".into());
    }
    if let Some(data) = bold {
        fonts.font_data.insert("ui-semibold".into(), std::sync::Arc::new(FontData::from_owned(data)));
        sb_family.insert(0, "ui-semibold".into());
    }
    sb_family.insert(0, "montserrat-semibold".into());

    // A chosen system font goes in front of Montserrat; its bold face (or, without
    // one, its regular face) serves the semibold family.
    if let Some((regular, bold)) = load_system_font(font) {
        fonts.font_data.insert("user".into(), std::sync::Arc::new(FontData::from_owned(regular)));
        fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "user".into());
        let bold_name = match bold {
            Some(data) => {
                fonts.font_data.insert("user-bold".into(), std::sync::Arc::new(FontData::from_owned(data)));
                "user-bold"
            }
            None => "user",
        };
        sb_family.insert(0, bold_name.into());
    }

    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), sb_family);
    if let Some(data) = monospace {
        fonts.font_data.insert("ui-mono".into(), std::sync::Arc::new(FontData::from_owned(data)));
        fonts.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, "ui-mono".into());
    }
    ctx.set_fonts(fonts);
}

/// Install fonts, colours and the egui style. Called at start-up and again
/// whenever the appearance settings change.
pub fn apply(ctx: &egui::Context, look: &Appearance) {
    setup_fonts(ctx, &look.font);
    apply_style(ctx, look);
}

/// Colours, corner radii and spacing without reloading the fonts.
pub fn apply_style(ctx: &egui::Context, look: &Appearance) {
    DARK.store(look.theme == ThemeMode::Dark, Ordering::Relaxed);
    let preset = ColorPreset::ALL.iter().position(|p| *p == look.preset()).unwrap_or(0);
    PRESET.store(preset as u8, Ordering::Relaxed);
    ROUNDING.store((f32::from(look.rounding.min(100)) / 100.0).to_bits(), Ordering::Relaxed);
    let dark = is_dark();

    let mut style = (*ctx.global_style()).clone();
    style.visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let v = &mut style.visuals;
    v.panel_fill = BG();
    v.window_fill = CARD();
    v.window_stroke = Stroke::new(1.0, BORDER());
    v.window_corner_radius = CornerRadius::same(rad_u8(16));
    v.menu_corner_radius = CornerRadius::same(rad_u8(14));
    v.extreme_bg_color = INPUT_BG();
    v.faint_bg_color = HOVER();
    v.code_bg_color = INPUT_BG();
    v.hyperlink_color = BLUE();
    v.selection.bg_fill = accent_soft(if dark { (40, 78, 136) } else { (198, 222, 255) }, 0.26, 0.42);
    v.selection.stroke = Stroke::new(1.0, BLUE());
    let shadow = if dark { 90 } else { 28 };
    v.window_shadow = egui::Shadow {
        offset: [0, 10],
        blur: 30,
        spread: 0,
        color: Color32::from_black_alpha(shadow),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(shadow + 2),
    };

    let w = &mut v.widgets;
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = CornerRadius::same(rad_u8(8));
        wv.fg_stroke = Stroke::new(1.0, TEXT());
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER());
    let inactive = surface(if dark { (46, 50, 58) } else { (241, 244, 249) });
    w.inactive.bg_fill = inactive;
    w.inactive.weak_bg_fill = inactive;
    w.inactive.bg_stroke = Stroke::NONE;
    let hovered = accent_soft(if dark { (56, 62, 74) } else { (229, 236, 247) }, 0.08, 0.14);
    w.hovered.bg_fill = hovered;
    w.hovered.weak_bg_fill = hovered;
    w.hovered.bg_stroke = Stroke::new(1.0, accent_soft(if dark { (76, 92, 122) } else { (208, 222, 244) }, 0.2, 0.38));
    let active = accent_soft(if dark { (40, 68, 116) } else { (214, 228, 250) }, 0.17, 0.36);
    w.active.bg_fill = active;
    w.active.weak_bg_fill = active;
    w.active.bg_stroke = Stroke::new(1.0, BLUE());
    if dark {
        w.open.bg_fill = hovered;
        w.open.weak_bg_fill = hovered;
        w.noninteractive.bg_fill = CARD();
        w.noninteractive.weak_bg_fill = CARD();
    }

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(BUTTON_PADDING_X, BUTTON_PADDING_Y);
    style.spacing.interact_size.y = 28.0;
    style.spacing.menu_margin = egui::Margin::same(8);
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
    // Both style slots are the same; the preference only drives the native title bar,
    // which follows the system in the light theme as before.
    ctx.set_theme(if dark { egui::ThemePreference::Dark } else { egui::ThemePreference::System });
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
            apply(&ctx, &Appearance::default());
            let input = egui::RawInput {
                system_theme: Some(system_theme),
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    assert_eq!(ui.spacing().button_padding, egui::vec2(BUTTON_PADDING_X, BUTTON_PADDING_Y));
                    let text = "Готово";
                    let text_size = ui.painter().layout_no_wrap(text.into(), semibold(14.0), TEXT()).size();
                    let response = ui.add(egui::Button::new(egui::RichText::new(text).font(semibold(14.0))));
                    assert!(response.rect.width() >= text_size.x + BUTTON_PADDING_X * 2.0);
                    assert!(response.rect.height() >= text_size.y + BUTTON_PADDING_Y * 2.0);
                });
            }).textures_delta.clear();
        }
    }
}
