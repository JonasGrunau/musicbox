//! Dark palette matching rekordbox's browser (colours sampled from its skin XML, no assets copied).

use egui::{Color32, Context, Visuals};

pub const BG_PANEL: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x1E);
pub const BG_WINDOW: Color32 = Color32::from_rgb(0x28, 0x28, 0x28);
pub const BG_HEADER: Color32 = Color32::from_rgb(0x25, 0x29, 0x2B);
pub const ACCENT: Color32 = Color32::from_rgb(0x14, 0x73, 0xEB);
pub const TEXT: Color32 = Color32::from_rgb(0xAA, 0xAA, 0xAA);

pub fn apply(ctx: &Context) {
    let mut visuals = Visuals::dark();
    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_WINDOW;
    visuals.extreme_bg_color = BG_HEADER;
    visuals.selection.bg_fill = ACCENT;
    visuals.override_text_color = Some(TEXT);
    ctx.set_visuals(visuals);
}

pub const TEXT_HEADER: Color32 = Color32::from_rgb(0x7D, 0x7D, 0x7D);

pub const STAR_ON: Color32 = Color32::from_rgb(0xFF, 0xB9, 0x37);
pub const STAR_OFF: Color32 = Color32::from_rgb(0x46, 0x46, 0x46);

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
