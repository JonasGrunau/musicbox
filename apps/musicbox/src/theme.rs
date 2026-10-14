//! musicbox's look: a dark palette in the spirit of rekordbox's browser (our own values, no skin
//! assets), the macOS system font loaded at run time, and the egui style. Design notes and the
//! token table: docs/design.md.

use egui::epaint::text::VariationCoords;
use egui::{Color32, Context, CornerRadius, FontData, FontFamily, FontId, FontTweak, Margin, Shadow, Stroke, TextStyle, Visuals};
use std::sync::Arc;

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

// Surfaces, darkest to lightest.
/// Seams between panels.
pub const SEAM: Color32 = rgb(0x0A0A0A);
/// Waveform wells and text fields.
pub const BG_WELL: Color32 = rgb(0x0F0F0F);
/// Tree and track table.
pub const BG_PANEL: Color32 = rgb(0x171717);
/// Odd table rows.
pub const BG_STRIPE: Color32 = rgb(0x1B1B1B);
/// Player area, top bar, status bar.
pub const BG_DECK: Color32 = rgb(0x1E1E1E);
/// Table header and browser toolbar.
pub const BG_BAR: Color32 = rgb(0x212121);
/// Buttons, menus and dialogs.
pub const BG_RAISED: Color32 = rgb(0x2A2A2A);
pub const BG_HOVER: Color32 = rgb(0x343434);
pub const BG_PRESSED: Color32 = rgb(0x3D3D3D);
/// Lines inside a surface (toolbar separators, header column edges).
pub const LINE: Color32 = rgb(0x2E2E2E);

pub const TEXT: Color32 = rgb(0xD6D6D6);
pub const TEXT_STRONG: Color32 = rgb(0xF2F2F2);
pub const TEXT_DIM: Color32 = rgb(0x9C9C9C);
pub const TEXT_MUTED: Color32 = rgb(0x6C6C6C);
pub const TEXT_HEADER: Color32 = rgb(0x8A8A8A);

pub const ACCENT: Color32 = rgb(0x1473EB);
pub const ACCENT_HOVER: Color32 = rgb(0x2A85F7);
/// Selected rows (table and tree).
pub const SELECTION: Color32 = rgb(0x163C66);
pub const ROW_HOVER: Color32 = rgb(0x222629);

pub const WARN: Color32 = rgb(0xFFB937);
pub const DANGER: Color32 = rgb(0xF0524F);

pub const STAR_ON: Color32 = rgb(0xFFB937);
pub const STAR_OFF: Color32 = rgb(0x3A3A3A);

// Shared sizes (4 px grid).
pub const RADIUS: u8 = 4;
pub const ROW_HEIGHT: f32 = 24.0;
pub const TOP_BAR: f32 = 40.0;
pub const STATUS_BAR: f32 = 26.0;

/// Named font families on top of egui's two: system font weights.
pub const MEDIUM: &str = "medium";
pub const SEMIBOLD: &str = "semibold";

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MEDIUM.into()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn apply(ctx: &Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, semibold(15.0)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(8.0, 4.0);
        s.button_padding = egui::vec2(10.0, 3.0);
        s.interact_size = egui::vec2(24.0, 22.0);
        s.menu_margin = Margin::same(6);
        s.window_margin = Margin::same(16);
        s.indent = 14.0;
        s.combo_height = 320.0;
        s.icon_width = 14.0;
        s.icon_width_inner = 8.0;
        s.icon_spacing = 6.0;
        s.scroll = egui::style::ScrollStyle::thin();
        style.visuals = visuals();
    });
}

fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.override_text_color = None;
    v.weak_text_color = Some(TEXT_MUTED);
    v.panel_fill = BG_PANEL;
    v.window_fill = rgb(0x242424);
    v.window_stroke = Stroke::new(1.0, rgb(0x383838));
    v.window_corner_radius = CornerRadius::same(10);
    v.window_shadow = Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(150) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(130) };
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = BG_WELL;
    v.text_edit_bg_color = Some(BG_WELL);
    v.faint_bg_color = BG_STRIPE;
    v.code_bg_color = BG_WELL;
    v.hyperlink_color = ACCENT_HOVER;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = SELECTION;
    v.selection.stroke = Stroke::new(1.0, ACCENT_HOVER);
    v.striped = true;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v.button_frame = true;
    v.disabled_alpha = 0.4;

    let radius = CornerRadius::same(RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = BG_PANEL;
    w.noninteractive.weak_bg_fill = BG_PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, SEAM);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = radius;

    w.inactive.bg_fill = BG_RAISED;
    w.inactive.weak_bg_fill = BG_RAISED;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = radius;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = BG_HOVER;
    w.hovered.weak_bg_fill = BG_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, rgb(0x474747));
    w.hovered.fg_stroke = Stroke::new(1.5, TEXT_STRONG);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = BG_PRESSED;
    w.active.weak_bg_fill = BG_PRESSED;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(2.0, TEXT_STRONG);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;

    w.open.bg_fill = BG_HOVER;
    w.open.weak_bg_fill = BG_HOVER;
    w.open.bg_stroke = Stroke::new(1.0, rgb(0x474747));
    w.open.fg_stroke = Stroke::new(1.0, TEXT_STRONG);
    w.open.corner_radius = radius;
    v
}

/// The system UI font (SF Pro, a variable font) at text optical size in three weights, with
/// Hiragino as the CJK fallback; egui's bundled fonts stay behind them for symbols. Elsewhere,
/// or when the files are missing, egui's defaults remain.
fn install_fonts(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();
    let Some(sf) = system_font("/System/Library/Fonts/SFNS.ttf") else {
        ctx.set_fonts(fonts);
        return;
    };
    let face = |weight: f32| {
        let tweak = FontTweak { coords: VariationCoords::new([(b"wght", weight), (b"opsz", 17.0)]), ..Default::default() };
        Arc::new(FontData { font: std::borrow::Cow::Borrowed(sf), index: 0, tweak })
    };
    fonts.font_data.insert("sf-regular".into(), face(410.0));
    fonts.font_data.insert("sf-medium".into(), face(510.0));
    fonts.font_data.insert("sf-semibold".into(), face(620.0));
    let mut fallbacks = Vec::new();
    if let Some(cjk) = system_font("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc") {
        fonts.font_data.insert("hiragino".into(), Arc::new(FontData::from_static(cjk)));
        fallbacks.push("hiragino".to_owned());
    }
    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |first: &str| [vec![first.to_owned()], fallbacks.clone(), defaults.clone()].concat();
    fonts.families.insert(FontFamily::Proportional, family("sf-regular"));
    fonts.families.insert(FontFamily::Name(MEDIUM.into()), family("sf-medium"));
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), family("sf-semibold"));
    if let Some(mono) = system_font("/System/Library/Fonts/SFNSMono.ttf") {
        fonts.font_data.insert("sf-mono".into(), Arc::new(FontData::from_static(mono)));
        fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "sf-mono".into());
    }
    ctx.set_fonts(fonts);
}

/// A system font file, read once per process and kept for its lifetime (the faces borrow it).
fn system_font(path: &'static str) -> Option<&'static [u8]> {
    /// (path, contents or `None` when unreadable)
    type Loaded = (&'static str, Option<&'static [u8]>);
    static CACHE: std::sync::Mutex<Vec<Loaded>> = std::sync::Mutex::new(Vec::new());
    let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, bytes)) = cache.iter().find(|(p, _)| *p == path) {
        return *bytes;
    }
    let bytes = std::fs::read(path).ok().map(|b| &*Box::leak(b.into_boxed_slice()));
    cache.push((path, bytes));
    bytes
}

/// A thin vertical line between groups in a toolbar row.
pub fn separator(ui: &mut egui::Ui) {
    let height = ui.spacing().interact_size.y - 8.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, LINE);
}

/// Small upper-case caption ("DEVICES", "BPM").
pub fn caption(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase()).font(semibold(10.0)).color(TEXT_MUTED).extra_letter_spacing(0.8)
}

/// Frame of a full-width bar (top bar, toolbars, status bar).
pub fn bar_frame(fill: Color32, x: i8) -> egui::Frame {
    egui::Frame::new().fill(fill).inner_margin(Margin::symmetric(x, 0))
}

/// The primary button of a dialog (accent filled).
pub fn primary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text).color(TEXT_STRONG).font(medium(13.0))).fill(ACCENT).min_size(egui::vec2(76.0, 26.0))
}

pub fn secondary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(text.to_owned()).min_size(egui::vec2(76.0, 26.0))
}

/// Track colour slots as named in `djmdColor` (1 Pink … 8 Purple).
/// TODO(docs/re): take the exact RGB values from the binary's colour table.
pub fn track_color(slot: u8) -> Option<Color32> {
    Some(match slot {
        1 => Color32::from_rgb(0xF8, 0x70, 0xC0),
        2 => Color32::from_rgb(0xE6, 0x2B, 0x2B),
        3 => Color32::from_rgb(0xF0, 0x8C, 0x28),
        4 => Color32::from_rgb(0xF0, 0xDC, 0x28),
        5 => Color32::from_rgb(0x3C, 0xC8, 0x3C),
        6 => Color32::from_rgb(0x28, 0xC8, 0xD2),
        7 => Color32::from_rgb(0x30, 0x6E, 0xF0),
        8 => Color32::from_rgb(0x96, 0x46, 0xE6),
        _ => return None,
    })
}

/// Display colour of a key: one hue per position on the circle of fifths (`key::wheel_index`,
/// minor and major of a position share it), so compatible keys look alike. Unknown keys: dim text.
pub fn key_color(stored: &str) -> Color32 {
    let index = mb_core::key::wheel_index(stored);
    if index == usize::MAX {
        return TEXT_DIM;
    }
    let hue = (index / 2) as f32 / 12.0;
    egui::ecolor::Hsva::new(hue, 0.42, 0.95, 1.0).into()
}
