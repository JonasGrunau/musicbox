//! Track overview drawn from rekordbox's own analysis files (docs/re/anlz.md):
//! PWV4 colour preview or PWV6 3-band preview from .EXT/.2EX, plus cue markers.

use mb_anlz::{AnlzFile, Tag};
use mb_anlz::tags::{CueKind, CueListExt, Wave3Band, WaveColorPreview};
use std::path::Path;

pub struct TrackWaves {
    pub color: Option<WaveColorPreview>,
    pub three_band: Option<Wave3Band>,
    pub cues: Vec<CueMark>,
}

pub struct CueMark {
    pub ms: u32,
    pub hot: Option<char>,
    pub color: egui::Color32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Rgb,
    ThreeBand,
}

const MEMORY_CUE: egui::Color32 = egui::Color32::from_rgb(0xE6, 0x2B, 0x2B);
const HOT_CUE_DEFAULT: egui::Color32 = egui::Color32::from_rgb(0x2D, 0xD7, 0x28);

fn read_sibling(dat: &Path, ext: &str) -> Option<AnlzFile> {
    let data = std::fs::read(dat.with_extension(ext)).ok()?;
    AnlzFile::parse(&data).ok()
}

/// Loads waveforms and cues for the track whose `ANLZ0000.DAT` is at `dat`.
pub fn load(dat: &Path) -> Option<TrackWaves> {
    let ext = read_sibling(dat, "EXT")?;
    let ex2 = read_sibling(dat, "2EX");
    let color = ext.sections_with(Tag::PWV4).next().and_then(|s| WaveColorPreview::decode(s).ok());
    let three_band = ex2
        .as_ref()
        .and_then(|f| f.sections_with(Tag::PWV6).next().and_then(|s| Wave3Band::decode(s).ok()));
    let mut cues = Vec::new();
    for list in ext.cue_lists_ext() {
        push_cues(&mut cues, &list);
    }
    Some(TrackWaves { color, three_band, cues })
}

fn push_cues(out: &mut Vec<CueMark>, list: &CueListExt) {
    for e in &list.entries {
        let hot = (list.kind == CueKind::Hot).then(|| e.hot_cue_letter()).flatten();
        let color = match (hot, e.color()) {
            (Some(_), Some((_, [r, g, b]))) => egui::Color32::from_rgb(r, g, b),
            (Some(_), None) => HOT_CUE_DEFAULT,
            (None, _) => MEMORY_CUE,
        };
        out.push(CueMark { ms: e.time_ms(), hot, color });
    }
}

/// Full-track overview with cue markers; `duration_ms` maps cue times onto the width.
pub fn show_overview(ui: &mut egui::Ui, waves: &TrackWaves, duration_ms: u32, style: Style) {
    let desired = egui::vec2(ui.available_width(), 56.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(0x12, 0x12, 0x12));
    let mid = rect.center().y;
    let half = rect.height() / 2.0 - 2.0;

    let column = |i: usize, n: usize, height: f32, color: egui::Color32| {
        let x0 = rect.left() + rect.width() * i as f32 / n as f32;
        let x1 = rect.left() + rect.width() * (i + 1) as f32 / n as f32;
        let h = (height * half).max(0.5);
        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, mid - h), egui::pos2(x1.max(x0 + 1.0), mid + h)), 0.0, color);
    };

    match (style, &waves.three_band, &waves.color) {
        (Style::ThreeBand, Some(bands), _) => {
            let n = bands.len();
            let peak = bands.data.iter().flat_map(|c| c.iter().copied()).max().unwrap_or(1).max(1) as f32;
            // Draw low, then mid, then high on top, each scaled to its own height.
            let palette = [
                egui::Color32::from_rgb(0x1A, 0x5F, 0xE8),
                egui::Color32::from_rgb(0xF0, 0x9A, 0x2C),
                egui::Color32::from_rgb(0xF4, 0xF4, 0xF4),
            ];
            for (band, colour) in palette.iter().enumerate() {
                for i in 0..n {
                    let v = bands.data[i][band] as f32 / peak;
                    column(i, n, v, *colour);
                }
            }
        }
        (_, _, Some(preview)) => {
            let n = preview.len();
            let peak = preview.max_height().max(1) as f32;
            for i in 0..n {
                if let Some(([r, g, b], h)) = preview.rgb_column(i) {
                    column(i, n, h as f32 / peak, egui::Color32::from_rgb(r, g, b));
                }
            }
        }
        _ => {}
    }

    if duration_ms > 0 {
        for cue in &waves.cues {
            let x = rect.left() + rect.width() * (cue.ms as f32 / duration_ms as f32).min(1.0);
            let top = rect.top();
            painter.line_segment([egui::pos2(x, top), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.0, cue.color));
            let marker = vec![egui::pos2(x - 4.0, top), egui::pos2(x + 4.0, top), egui::pos2(x, top + 6.0)];
            painter.add(egui::Shape::convex_polygon(marker, cue.color, egui::Stroke::NONE));
            if let Some(letter) = cue.hot {
                painter.text(
                    egui::pos2(x + 3.0, top + 6.0),
                    egui::Align2::LEFT_TOP,
                    letter,
                    egui::FontId::proportional(9.0),
                    egui::Color32::BLACK,
                );
            }
        }
    }
}
