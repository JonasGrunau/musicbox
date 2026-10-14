//! Drawing pieces of the player area at the top (the `App::deck` logic lives in `app.rs`):
//! artwork, title block, BPM/key/time readouts, the play button, the waveform style switch, and
//! the editing switch of the top bar. Layout: docs/design.md § Deck.

use crate::icons::{self, Icon};
use crate::theme;
use crate::waveform;
use mb_core::Track;
use mb_core::key::{KeyNotation, display as key_display};

/// Height of the zoomed waveform strip.
pub const DETAIL_HEIGHT: f32 = 64.0;
/// Height of the artwork / title / overview row.
pub const INFO_HEIGHT: f32 = 76.0;
/// Height of one row of cue or grid controls.
pub const CONTROL_HEIGHT: f32 = 26.0;
/// Vertical padding of the deck and the gap between its rows.
pub const PAD: f32 = 10.0;

/// Deck height for what it shows: no track or a device/xml track (waveforms and info), or a
/// collection track (also the two control rows).
pub fn height(controls: bool) -> f32 {
    let base = PAD + DETAIL_HEIGHT + PAD + INFO_HEIGHT + PAD;
    if controls { base + 2.0 * CONTROL_HEIGHT + 6.0 + PAD } else { base }
}

/// Track artwork, preferring the 240 px `_m` variant that rekordbox writes next to the small one.
pub fn artwork(ui: &mut egui::Ui, path: Option<&std::path::Path>, size: f32) {
    let larger = path.map(|p| {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        p.with_file_name(format!("{stem}_m.jpg"))
    });
    let chosen = larger.filter(|p| p.is_file()).or_else(|| path.map(|p| p.to_path_buf())).filter(|p| p.is_file());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    match chosen {
        Some(p) => {
            egui::Image::new(format!("file://{}", p.display())).corner_radius(4.0).paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, 4.0, theme::BG_RAISED);
            icons::paint(ui.painter(), rect.shrink(size * 0.3), Icon::Collection, theme::TEXT_MUTED);
        }
    }
}

/// Title and artist lines; the empty deck shows placeholders.
pub fn title_block(ui: &mut egui::Ui, track: Option<&Track>) {
    let (title, artist) = match track {
        Some(t) => (t.title.as_str(), t.artist.as_str()),
        None => ("No track selected", "Select a track to preview it"),
    };
    let title_color = if track.is_some() { theme::TEXT_STRONG } else { theme::TEXT_MUTED };
    ui.add(egui::Label::new(egui::RichText::new(title).font(theme::semibold(16.0)).color(title_color)).truncate().selectable(false));
    let mut line = artist.to_owned();
    if let Some(t) = track.filter(|t| !t.album.is_empty()) {
        line = if line.is_empty() { t.album.clone() } else { format!("{line}  ·  {}", t.album) };
    }
    ui.add(egui::Label::new(egui::RichText::new(line).color(theme::TEXT_DIM)).truncate().selectable(false));
}

fn clock(ms: f64) -> String {
    let s = (ms / 1000.0).max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// BPM, key and time, each as a caption over a large value. Time counts down while a position is
/// set, else shows the length.
pub fn readouts(ui: &mut egui::Ui, track: Option<&Track>, notation: KeyNotation, position_ms: f64, duration_ms: u32) {
    let bpm = track.and_then(Track::bpm).map_or_else(|| "–".to_owned(), |b| format!("{b:.2}"));
    let (key, key_color) = match track.filter(|t| !t.key.is_empty()) {
        Some(t) => (key_display(&t.key, notation), theme::key_color(&t.key)),
        None => ("–".to_owned(), theme::TEXT_MUTED),
    };
    let time = match (track, duration_ms) {
        (None, _) | (_, 0) => "–".to_owned(),
        (Some(_), d) if position_ms > 0.0 => format!("-{}", clock(f64::from(d) - position_ms)),
        (Some(_), d) => clock(f64::from(d)),
    };
    let value_color = if track.is_some() { theme::TEXT_STRONG } else { theme::TEXT_MUTED };
    for (caption, value, color, width) in [("Time", time, value_color, 64.0), ("Key", key, key_color, 52.0), ("BPM", bpm, value_color, 78.0)] {
        ui.allocate_ui_with_layout(egui::vec2(width, 42.0), egui::Layout::top_down(egui::Align::Max), |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(theme::caption(caption));
            ui.add(egui::Label::new(egui::RichText::new(value).font(theme::semibold(19.0)).color(color)).selectable(false));
        });
    }
}

/// The round play / pause button left of the overview.
pub fn play_button(ui: &mut egui::Ui, playing: bool, enabled: bool) -> egui::Response {
    let size = 32.0;
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), sense);
    let label = if playing { "Pause" } else { "Play" };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let fill = match () {
        _ if playing => theme::ACCENT,
        _ if response.is_pointer_button_down_on() => theme::BG_PRESSED,
        _ if response.hovered() => theme::BG_HOVER,
        _ => theme::BG_RAISED,
    };
    ui.painter().circle_filled(rect.center(), size / 2.0, fill);
    let icon = if playing { Icon::Pause } else { Icon::Play };
    let color = if enabled { theme::TEXT_STRONG } else { theme::TEXT_MUTED };
    // The triangle's optical centre sits right of its box centre.
    let nudge = if playing { 0.0 } else { 1.0 };
    icons::paint(ui.painter(), egui::Rect::from_center_size(rect.center() + egui::vec2(nudge, 0.0), egui::vec2(13.0, 13.0)), icon, color);
    response
}

/// Two-way switch between the RGB and the 3-band overview.
pub fn wave_style_switch(ui: &mut egui::Ui, style: &mut waveform::Style) {
    let options = [(waveform::Style::Rgb, "RGB"), (waveform::Style::ThreeBand, "3-Band")];
    let width = 104.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 11.0, theme::BG_WELL);
    let half = width / 2.0;
    for (i, (value, label)) in options.into_iter().enumerate() {
        let cell = egui::Rect::from_min_size(rect.min + egui::vec2(i as f32 * half, 0.0), egui::vec2(half, rect.height())).shrink(2.0);
        let response = ui.interact(cell, ui.id().with(("wave-style", i)), egui::Sense::click());
        let on = *style == value;
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, label));
        if on {
            ui.painter().rect_filled(cell, 9.0, theme::BG_HOVER);
        }
        let color = if on { theme::TEXT_STRONG } else if response.hovered() { theme::TEXT } else { theme::TEXT_MUTED };
        ui.painter().text(cell.center(), egui::Align2::CENTER_CENTER, label, theme::medium(11.0), color);
        if response.clicked() {
            *style = value;
        }
    }
}

/// An empty waveform well with a centred note ("Not analysed").
pub fn empty_well(ui: &mut egui::Ui, height: f32, note: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
    ui.painter().rect_filled(rect, 3.0, theme::BG_WELL);
    ui.painter().hline(rect.x_range().shrink(12.0), rect.center().y, egui::Stroke::new(1.0, theme::LINE));
    if !note.is_empty() {
        let galley = ui.painter().layout_no_wrap(note.to_owned(), egui::FontId::proportional(11.5), theme::TEXT_MUTED);
        let bg = egui::Rect::from_center_size(rect.center(), galley.size() + egui::vec2(16.0, 6.0));
        ui.painter().rect_filled(bg, 4.0, theme::BG_WELL);
        ui.painter().galley(bg.min + egui::vec2(8.0, 3.0), galley, theme::TEXT_MUTED);
    }
}

/// State of the editing switch in the top bar.
pub enum EditState {
    ReadOnly,
    Starting,
    Editing,
}

/// The lock switch: read-only (locked) or editing (unlocked, accent). Returns the click.
pub fn editing_switch(ui: &mut egui::Ui, state: EditState) -> egui::Response {
    let (label, icon, on) = match state {
        EditState::ReadOnly => ("Read-only", Icon::Lock, false),
        EditState::Starting => ("Editing (starting…)", Icon::Unlock, true),
        EditState::Editing => ("Editing", Icon::Unlock, true),
    };
    let font = theme::medium(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, theme::TEXT);
    let size = egui::vec2(galley.size().x + 44.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, label));
    let (fill, stroke, fg) = match (on, response.hovered()) {
        (true, _) => (theme::SELECTION, theme::ACCENT, theme::TEXT_STRONG),
        (false, true) => (theme::BG_HOVER, theme::BG_HOVER, theme::TEXT_STRONG),
        (false, false) => (theme::BG_RAISED, theme::BG_RAISED, theme::TEXT),
    };
    ui.painter().rect(rect, 14.0, fill, egui::Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 18.0, rect.center().y), egui::vec2(12.0, 12.0));
    icons::paint(ui.painter(), icon_rect, icon, if on { theme::ACCENT_HOVER } else { theme::TEXT_DIM });
    ui.painter().galley_with_override_text_color(egui::pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    response
}
