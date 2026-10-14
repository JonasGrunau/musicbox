//! Cue and beat-grid editing on the deck. Every control sends one `mb_library` command, so undo
//! and redo work as for the other edits. Specs: docs/re/ops/edit-cues.md, edit-grid.md.

use crate::actions::Action;
use crate::waveform::{self, CueMark, HOT_CUE_COLORS, MEMORY_CUE_COLORS};
use crate::{deck, theme};
use mb_core::{DbId, Track};
use mb_library::{Beat, Command, Cue, CueEdit, CueSlot, NewCue, grid};

/// Hot-cue pads shown on the deck (A–H, as on the players).
pub const PADS: u8 = 8;

/// Deck editing state of the selected track.
#[derive(Default)]
pub struct State {
    /// Position used when no preview player is open for the track.
    pub cursor_ms: f64,
    /// Selected cue (`djmdCue.ID`).
    pub selected: Option<String>,
    /// Comment being edited, for `selected`.
    pub comment: String,
    pub bpm: String,
    track: Option<DbId>,
}

impl State {
    /// Resets the per-track fields when the selected track changes.
    pub fn follow(&mut self, track: &DbId, bpm: Option<f64>) {
        if self.track.as_ref() != Some(track) {
            *self = State { track: Some(track.clone()), bpm: bpm.map(|b| format!("{b:.2}")).unwrap_or_default(), ..State::default() };
        }
    }
}

fn format_ms(ms: u32) -> String {
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

fn label(cue: &Cue) -> String {
    let what = match cue.slot.letter() {
        Some(l) => format!("Hot {l}"),
        None if cue.out_ms.is_some() => "Memory loop".into(),
        None => "Memory".into(),
    };
    format!("{what} {}", format_ms(cue.in_ms))
}

/// Markers for the waveforms.
pub fn marks(cues: &[Cue]) -> Vec<CueMark> {
    cues.iter()
        .map(|c| CueMark {
            ms: c.in_ms,
            end_ms: c.out_ms,
            hot: c.slot.letter(),
            color: match c.slot {
                CueSlot::Hot(_) => waveform::hot_cue_color(c.color_table_index),
                CueSlot::Memory => waveform::memory_cue_color(c.color),
            },
        })
        .collect()
}

/// Track length for grid edits: the scroll wave's length when known, else the tag length.
pub fn duration_ms(track: &Track, waves: Option<&waveform::TrackWaves>) -> u32 {
    let tag = track.length_seconds.unwrap_or(0) * 1000;
    let wave = waves
        .and_then(|w| w.scroll.as_ref())
        .map(|s| (s.len() as u64 * 1000 / u64::from(s.columns_per_second.max(1))) as u32)
        .unwrap_or(0);
    tag.max(wave)
}

/// The two control rows under the waveforms. `position_ms` is the playhead or cursor; `jump`
/// receives a position to move to (a clicked pad).
#[allow(clippy::too_many_arguments)]
pub fn controls(
    ui: &mut egui::Ui,
    track: &Track,
    cues: &[Cue],
    beats: &[Beat],
    duration_ms: u32,
    position_ms: f64,
    editing: bool,
    state: &mut State,
    jump: &mut Option<f64>,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let pos = position_ms.max(0.0).round() as u32;
    let content = track.id.clone();
    let run = |actions: &mut Vec<Action>, command: Command| actions.push(Action::Run(command));
    if state.selected.as_ref().is_some_and(|id| !cues.iter().any(|c| &c.id == id)) {
        state.selected = None;
    }

    let row = |ui: &mut egui::Ui, caption: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), deck::CONTROL_HEIGHT), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.spacing_mut().interact_size.y = deck::CONTROL_HEIGHT;
            ui.spacing_mut().button_padding.y = 5.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(CAPTION_WIDTH, deck::CONTROL_HEIGHT), egui::Sense::hover());
            ui.painter().text(egui::pos2(rect.left(), rect.center().y), egui::Align2::LEFT_CENTER, caption.to_uppercase(), theme::semibold(10.0), theme::TEXT_MUTED);
            add(ui);
        });
    };

    row(ui, "Cue", &mut |ui| {
        ui.add_enabled_ui(editing, |ui| {
            if ui.button("Memory Cue").on_hover_text("Add a memory cue at the position").clicked() {
                let cue = NewCue { slot: CueSlot::Memory, in_ms: pos, out_ms: None, color: None, color_table_index: None, comment: String::new() };
                run(&mut actions, Command::AddCue { content: content.clone(), cue });
            }
        });
        theme::separator(ui);
        ui.spacing_mut().item_spacing.x = 4.0;
        for pad in 0..PADS {
            let letter = (b'A' + pad) as char;
            let existing = cues.iter().find(|c| c.slot == CueSlot::Hot(pad));
            let response = hot_cue_pad(ui, letter, existing, state.selected.as_ref());
            if response.clicked() {
                match existing {
                    Some(c) => {
                        state.selected = Some(c.id.clone());
                        state.comment = c.comment.clone();
                        *jump = Some(f64::from(c.in_ms));
                    }
                    None if editing => {
                        let cue = NewCue { slot: CueSlot::Hot(pad), in_ms: pos, out_ms: None, color: None, color_table_index: None, comment: String::new() };
                        run(&mut actions, Command::AddCue { content: content.clone(), cue });
                    }
                    None => {}
                }
            }
            if let Some(c) = existing {
                response.context_menu(|ui| {
                    if ui.add_enabled(editing, egui::Button::new("Delete Hot Cue")).clicked() {
                        actions.push(Action::Run(Command::DeleteCue { id: c.id.clone() }));
                        ui.close();
                    }
                });
            }
        }
        ui.spacing_mut().item_spacing.x = 6.0;
        theme::separator(ui);
        let current = state.selected.as_ref().and_then(|id| cues.iter().find(|c| &c.id == id));
        egui::ComboBox::from_id_salt("cue-list")
            .width(170.0)
            .selected_text(current.map_or_else(|| format!("{} cue(s)", cues.len()), label))
            .show_ui(ui, |ui| {
                for c in cues {
                    if ui.selectable_label(Some(&c.id) == state.selected.as_ref(), label(c)).clicked() {
                        state.selected = Some(c.id.clone());
                        state.comment = c.comment.clone();
                        *jump = Some(f64::from(c.in_ms));
                    }
                }
            });
        if let Some(c) = current {
            ui.add_enabled_ui(editing, |ui| {
                cue_colour_menu(ui, c, &mut actions);
                ui.add(egui::TextEdit::singleline(&mut state.comment).hint_text("Comment").desired_width(170.0).margin(egui::vec2(8.0, 4.0)));
                if ui.button("Save Comment").clicked() && state.comment != c.comment {
                    actions.push(Action::Run(Command::EditCue { id: c.id.clone(), edit: CueEdit::Comment(state.comment.clone()) }));
                }
                if ui.button("Delete Cue").clicked() {
                    actions.push(Action::Run(Command::DeleteCue { id: c.id.clone() }));
                }
            });
        }
    });

    ui.add_space(6.0);
    row(ui, "Grid", &mut |ui| {
        ui.add_enabled_ui(editing && !beats.is_empty(), |ui| {
            let step = if ui.input(|i| i.modifiers.shift) { 10 } else { 1 };
            if ui.button("◀ Grid").on_hover_text("Move the grid 1 ms earlier (⇧: 10 ms)").clicked() {
                run(&mut actions, Command::SetBeatGrid { content: content.clone(), beats: grid::shift(beats, -step, duration_ms) });
            }
            if ui.button("Grid ▶").on_hover_text("Move the grid 1 ms later (⇧: 10 ms)").clicked() {
                run(&mut actions, Command::SetBeatGrid { content: content.clone(), beats: grid::shift(beats, step, duration_ms) });
            }
            if ui.button("Set Downbeat").on_hover_text("The beat nearest the position becomes beat 1").clicked()
                && let Some(i) = grid::nearest_beat(beats, position_ms)
            {
                run(&mut actions, Command::SetBeatGrid { content: content.clone(), beats: grid::set_downbeat(beats, i) });
            }
            theme::separator(ui);
            ui.add(egui::TextEdit::singleline(&mut state.bpm).hint_text("BPM").desired_width(56.0).margin(egui::vec2(8.0, 4.0)).horizontal_align(egui::Align::Max));
            if ui.button("Set BPM").clicked() {
                let parsed = state.bpm.trim().replace(',', ".").parse::<f64>().ok().map(|b| (b * 100.0).round());
                if let (Some(tempo), Some(anchor)) = (parsed, grid::nearest_beat(beats, position_ms))
                    && tempo > 0.0
                    && tempo <= f64::from(u16::MAX)
                    && let Ok(new) = grid::set_tempo(beats, anchor, tempo as u16, duration_ms)
                {
                    run(&mut actions, Command::SetBeatGrid { content: content.clone(), beats: new });
                }
            }
            if ui.button("BPM ×2").clicked()
                && let Ok(new) = grid::double(beats, duration_ms)
            {
                run(&mut actions, Command::SetBeatGrid { content: content.clone(), beats: new });
            }
            if ui.button("BPM ÷2").clicked()
                && let Ok(new) = grid::halve(beats)
            {
                run(&mut actions, Command::SetBeatGrid { content: content.clone(), beats: new });
            }
        });
        // The tempo where the cursor is: dynamic grids change tempo along the track.
        if let Some(b) = grid::nearest_beat(beats, position_ms).and_then(|i| beats.get(i)) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = format!("{:.2} BPM at the cursor  ·  {} beats", b.bpm(), beats.len());
                ui.label(egui::RichText::new(text).small().color(theme::TEXT_MUTED));
            });
        }
    });
    actions
}

/// Width of the "CUE" / "GRID" captions, so both rows start their controls at the same x.
const CAPTION_WIDTH: f32 = 34.0;

/// One hot-cue pad: filled with the cue's colour when set (dark letter), an empty tile when not;
/// the selected cue's pad gets a white ring.
fn hot_cue_pad(ui: &mut egui::Ui, letter: char, cue: Option<&Cue>, selected: Option<&String>) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, deck::CONTROL_HEIGHT), egui::Sense::click());
    let text = letter.to_string();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &text));
    let painter = ui.painter();
    match cue {
        Some(c) => {
            let color = waveform::hot_cue_color(c.color_table_index);
            let fill = if response.hovered() { color } else { color.gamma_multiply(0.85) };
            painter.rect_filled(rect, theme::RADIUS, fill);
            if selected == Some(&c.id) {
                painter.rect_stroke(rect.expand(1.5), theme::RADIUS + 1, egui::Stroke::new(1.5, theme::TEXT_STRONG), egui::StrokeKind::Inside);
            }
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, theme::semibold(12.5), egui::Color32::from_rgb(0x10, 0x10, 0x10));
        }
        None => {
            let fill = if response.hovered() { theme::BG_HOVER } else { theme::BG_RAISED };
            painter.rect_filled(rect, theme::RADIUS, fill);
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, theme::semibold(12.5), theme::TEXT_MUTED);
        }
    }
    response
}

/// Colour menu of the selected cue: colour codes for hot cues, colour slots for memory cues.
fn cue_colour_menu(ui: &mut egui::Ui, cue: &Cue, actions: &mut Vec<Action>) {
    let swatch = match cue.slot {
        CueSlot::Hot(_) => waveform::hot_cue_color(cue.color_table_index),
        CueSlot::Memory => waveform::memory_cue_color(cue.color),
    };
    ui.menu_button(egui::RichText::new("■ Colour").color(swatch), |ui| match cue.slot {
        CueSlot::Hot(_) => {
            for (code, [r, g, b]) in HOT_CUE_COLORS {
                if ui.button(egui::RichText::new(format!("■ {code}")).color(egui::Color32::from_rgb(r, g, b))).clicked() {
                    actions.push(Action::Run(Command::EditCue { id: cue.id.clone(), edit: CueEdit::ColorTableIndex(code) }));
                    ui.close();
                }
            }
        }
        CueSlot::Memory => {
            if ui.button("No colour").clicked() {
                actions.push(Action::Run(Command::EditCue { id: cue.id.clone(), edit: CueEdit::Color(None) }));
                ui.close();
            }
            for (slot, [r, g, b]) in MEMORY_CUE_COLORS.iter().enumerate() {
                if ui.button(egui::RichText::new(format!("■ {}", slot + 1)).color(egui::Color32::from_rgb(*r, *g, *b))).clicked() {
                    actions.push(Action::Run(Command::EditCue { id: cue.id.clone(), edit: CueEdit::Color(Some(slot as u8)) }));
                    ui.close();
                }
            }
        }
    });
}
