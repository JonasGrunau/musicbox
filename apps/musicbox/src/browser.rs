//! Track table: filtered, sorted, virtualized.

use crate::app::{LibraryRef, List, Source};
use crate::theme;
use egui_extras::{Column, TableBuilder};
use mb_core::key::{KeyNotation, display as key_display, wheel_index};
use mb_core::{LibrarySnapshot, Track};
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Index,
    Title,
    Artist,
    Album,
    Genre,
    Bpm,
    Key,
    Time,
    Rating,
    DateAdded,
    Bitrate,
    Kind,
    Color,
}

const COLUMNS: &[(Col, &str, f32)] = &[
    (Col::Index, "#", 40.0),
    (Col::Color, "", 18.0),
    (Col::Title, "Title", 260.0),
    (Col::Artist, "Artist", 180.0),
    (Col::Album, "Album", 160.0),
    (Col::Genre, "Genre", 110.0),
    (Col::Bpm, "BPM", 56.0),
    (Col::Key, "Key", 48.0),
    (Col::Time, "Time", 52.0),
    (Col::Rating, "Rating", 70.0),
    (Col::DateAdded, "Date Added", 90.0),
    (Col::Bitrate, "Bitrate", 70.0),
    (Col::Kind, "Kind", 50.0),
];

pub struct State {
    pub search: String,
    pub sort: Col,
    pub ascending: bool,
    pub key_notation: KeyNotation,
    /// Indices into `LibrarySnapshot::tracks`, in display order.
    rows: Vec<usize>,
    built_for: Option<(Source, String, Col, bool)>,
    /// (library, track index) of the selected row.
    pub selected: Option<(LibraryRef, usize)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            search: String::new(),
            sort: Col::Index,
            ascending: true,
            key_notation: KeyNotation::Alphanumeric,
            rows: Vec::new(),
            built_for: None,
            selected: None,
        }
    }
}

impl State {
    pub fn invalidate(&mut self) {
        self.built_for = None;
    }

    pub fn visible_count(&self) -> usize {
        self.rows.len()
    }

    fn rebuild(&mut self, library: &LibrarySnapshot, source: &Source) {
        let key = (source.clone(), self.search.clone(), self.sort, self.ascending);
        if self.built_for.as_ref() == Some(&key) {
            return;
        }
        let mut rows: Vec<usize> = match &source.list {
            List::Collection => (0..library.tracks.len()).collect(),
            List::Playlist(id) | List::History(id) | List::MyTag(id) => {
                let entries = match &source.list {
                    List::Playlist(_) => &library.playlist_entries,
                    List::History(_) => &library.history_entries,
                    _ => &library.my_tag_entries,
                };
                let by_id: HashMap<_, _> = library.tracks.iter().enumerate().map(|(i, t)| (&t.id, i)).collect();
                entries
                    .get(id)
                    .map(|ids| ids.iter().filter_map(|id| by_id.get(id).copied()).collect())
                    .unwrap_or_default()
            }
        };
        let needle = self.search.to_lowercase();
        if !needle.is_empty() {
            rows.retain(|&i| matches_search(&library.tracks[i], &needle));
        }
        if self.sort != Col::Index {
            let tracks = &library.tracks;
            rows.sort_by(|&a, &b| compare(&tracks[a], &tracks[b], self.sort));
        }
        if !self.ascending {
            rows.reverse();
        }
        self.rows = rows;
        self.built_for = Some(key);
    }
}

fn matches_search(t: &Track, needle: &str) -> bool {
    [&t.title, &t.artist, &t.album, &t.genre, &t.label, &t.comment, &t.remixer]
        .iter()
        .any(|field| field.to_lowercase().contains(needle))
}

fn compare(a: &Track, b: &Track, col: Col) -> Ordering {
    let text = |x: &str, y: &str| x.to_lowercase().cmp(&y.to_lowercase());
    match col {
        Col::Index => Ordering::Equal,
        Col::Title => text(&a.title, &b.title),
        Col::Artist => text(&a.artist, &b.artist),
        Col::Album => text(&a.album, &b.album),
        Col::Genre => text(&a.genre, &b.genre),
        Col::Bpm => a.bpm_x100.cmp(&b.bpm_x100),
        Col::Key => wheel_index(&a.key).cmp(&wheel_index(&b.key)),
        Col::Time => a.length_seconds.cmp(&b.length_seconds),
        Col::Rating => a.rating.cmp(&b.rating),
        Col::DateAdded => a.date_added.cmp(&b.date_added),
        Col::Bitrate => a.bitrate.cmp(&b.bitrate),
        Col::Kind => a.file_type.label().cmp(b.file_type.label()),
        Col::Color => a.color.cmp(&b.color),
    }
}

fn cell_text(t: &Track, col: Col, index: usize, notation: KeyNotation) -> String {
    match col {
        Col::Index => (index + 1).to_string(),
        Col::Title => t.title.clone(),
        Col::Artist => t.artist.clone(),
        Col::Album => t.album.clone(),
        Col::Genre => t.genre.clone(),
        Col::Bpm => t.bpm().map(|b| format!("{b:.2}")).unwrap_or_default(),
        Col::Key => key_display(&t.key, notation),
        Col::Time => t.length_seconds.map(|s| format!("{}:{:02}", s / 60, s % 60)).unwrap_or_default(),
        Col::Rating | Col::Color => String::new(),
        Col::DateAdded => t.date_added.clone(),
        Col::Bitrate => t.bitrate.map(|b| format!("{b} kbps")).unwrap_or_default(),
        Col::Kind => t.file_type.label().to_owned(),
    }
}

pub fn show(ui: &mut egui::Ui, library: &LibrarySnapshot, source: &Source, state: &mut State) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.search)
                .hint_text("Search")
                .desired_width(260.0),
        );
        if !state.search.is_empty() && ui.small_button("✕").clicked() {
            state.search.clear();
        }
    });
    ui.add_space(4.0);
    state.rebuild(library, source);

    let row_height = 20.0;
    let mut table = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .min_scrolled_height(0.0)
        .auto_shrink(false);
    for (_, _, width) in COLUMNS {
        table = table.column(Column::initial(*width).at_least(24.0).clip(true));
    }
    let mut clicked_header = None;
    table
        .header(row_height, |mut header| {
            for (col, label, _) in COLUMNS {
                header.col(|ui| {
                    let text = egui::RichText::new(*label).color(theme::TEXT_HEADER);
                    if ui.add(egui::Label::new(text).sense(egui::Sense::click())).clicked() {
                        clicked_header = Some(*col);
                    }
                    if state.sort == *col && *col != Col::Index {
                        sort_arrow(ui, state.ascending);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(row_height, state.rows.len(), |mut row| {
                let display_index = row.index();
                let track_index = state.rows[display_index];
                let track = &library.tracks[track_index];
                row.set_selected(state.selected == Some((source.library, track_index)));
                for (col, _, _) in COLUMNS {
                    row.col(|ui| match col {
                        Col::Rating => stars(ui, track.rating),
                        Col::Color => color_dot(ui, track.color),
                        _ => {
                            ui.add(egui::Label::new(cell_text(track, *col, display_index, state.key_notation)).truncate().selectable(false));
                        }
                    });
                }
                if row.response().clicked() {
                    state.selected = Some((source.library, track_index));
                }
            });
        });
    if let Some(col) = clicked_header {
        if state.sort == col {
            state.ascending = !state.ascending;
        } else {
            state.sort = col;
            state.ascending = true;
        }
    }
}

/// Small filled triangle after the header label (the default fonts lack ▲/▼).
fn sort_arrow(ui: &mut egui::Ui, ascending: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    let c = rect.center();
    let (tip, base) = if ascending { (-3.0, 3.0) } else { (3.0, -3.0) };
    ui.painter().add(egui::Shape::convex_polygon(
        vec![egui::pos2(c.x, c.y + tip), egui::pos2(c.x + 4.0, c.y + base), egui::pos2(c.x - 4.0, c.y + base)],
        theme::TEXT_HEADER,
        egui::Stroke::NONE,
    ));
}

/// Five painted stars, filled up to `rating` (0..=5).
fn stars(ui: &mut egui::Ui, rating: u8) {
    let size = 10.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size * 5.0 + 4.0, size), egui::Sense::hover());
    for i in 0..5 {
        let center = egui::pos2(rect.left() + size / 2.0 + i as f32 * (size + 1.0), rect.center().y);
        let color = if i < rating { theme::STAR_ON } else { theme::STAR_OFF };
        star(ui.painter(), center, size / 2.0, color);
    }
}

/// A five-pointed star as convex pieces (inner pentagon + five tip triangles) so it fills cleanly.
fn star(painter: &egui::Painter, c: egui::Pos2, r: f32, color: egui::Color32) {
    let point = |k: usize, radius: f32| {
        let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
        egui::pos2(c.x + radius * a.cos(), c.y + radius * a.sin())
    };
    let inner_r = r * 0.42;
    let inner: Vec<_> = (0..5).map(|k| point(2 * k + 1, inner_r)).collect();
    painter.add(egui::Shape::convex_polygon(inner.clone(), color, egui::Stroke::NONE));
    for k in 0..5 {
        let tip = point(2 * k, r);
        let left = inner[(k + 4) % 5];
        let right = inner[k];
        painter.add(egui::Shape::convex_polygon(vec![tip, right, left], color, egui::Stroke::NONE));
    }
}

/// rekordbox track colour slot (1 Pink … 8 Purple, `djmdColor`) as a dot.
fn color_dot(ui: &mut egui::Ui, color: Option<u8>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    if let Some(c) = color.and_then(theme::track_color) {
        ui.painter().circle_filled(rect.center(), 4.0, c);
    }
}
