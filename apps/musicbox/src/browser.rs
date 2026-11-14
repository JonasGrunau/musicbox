//! Track table: filtered, sorted, virtualized.

use crate::actions::{Action, TrackDrag};
use crate::app::{LibraryRef, List, Source};
use crate::icons::{self, Icon};
use crate::theme;
use egui_extras::{Column, TableBuilder};
use mb_core::key::{KeyNotation, display as key_display, wheel_index};
use mb_core::{LibrarySnapshot, Track};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use mb_core::{DbId, PlaylistKind, PlaylistNode};
use mb_library::{Command, TrackEdit};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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
    Artwork,
}

const COLUMNS: &[(Col, &str, f32)] = &[
    (Col::Color, "", 6.0),
    (Col::Index, "#", 30.0),
    (Col::Artwork, "", 18.0),
    (Col::Title, "Title", 260.0),
    (Col::Artist, "Artist", 150.0),
    (Col::Album, "Album", 120.0),
    (Col::Genre, "Genre", 84.0),
    (Col::Bpm, "BPM", 54.0),
    (Col::Key, "Key", 40.0),
    (Col::Time, "Time", 42.0),
    (Col::Rating, "Rating", 64.0),
    (Col::DateAdded, "Date Added", 78.0),
    (Col::Bitrate, "Bitrate", 70.0),
    (Col::Kind, "Kind", 40.0),
];

/// Gap between columns.
const COLUMN_GAP: f32 = 8.0;
/// Title width the narrow-browser scaling aims to keep.
const TITLE_COMFORT: f32 = 220.0;

impl Col {
    /// Numbers line up on the right, like the players' lists.
    fn right_aligned(self) -> bool {
        matches!(self, Col::Index | Col::Bpm | Col::Time | Col::Bitrate)
    }

    /// Text columns that start narrower in a narrow browser.
    fn shrinks(self) -> bool {
        matches!(self, Col::Artist | Col::Album | Col::Genre)
    }

    /// Columns with no text of their own (painted cells) do not sort or resize by a label.
    fn painted(self) -> bool {
        matches!(self, Col::Color | Col::Artwork)
    }
}

pub struct State {
    pub search: String,
    pub sort: Col,
    pub ascending: bool,
    pub key_notation: KeyNotation,
    /// Indices into `LibrarySnapshot::tracks`, in display order.
    rows: Vec<usize>,
    /// What `rows` was built for: source, search, sort, direction, and the snapshot's address
    /// and track count (a device reload swaps the snapshot under the same source).
    built_for: Option<(Source, String, Col, bool, usize, usize)>,
    /// (library, track index) of the focused row; the deck shows this track.
    pub selected: Option<(LibraryRef, usize)>,
    /// Track indices of all selected rows (multi-select).
    pub selection: BTreeSet<usize>,
    /// Display index where the last plain/⌘ click happened, for ⇧-click ranges.
    anchor: Option<usize>,
    /// For playlist views: position of each displayed row within the playlist's entries.
    entry_pos: Vec<Option<usize>>,
    /// The rows were re-sorted or re-filtered: bring the focused track back into view once.
    reveal_selected: bool,
    /// Another list is shown: start at its top.
    scroll_to_top: bool,
    lookups: Lookups,
    thumbs: Thumbs,
}

/// Per-snapshot lookups, built once per snapshot and reused by every rebuild and frame, so that
/// typing a search, sorting and switching lists stay fast on large collections.
#[derive(Default)]
struct Lookups {
    /// The snapshot these belong to: address, track count, first and last track ID. A reloaded
    /// library can sit at the same address (the app's own field), so `State::invalidate` also
    /// drops them.
    built_for: Option<(usize, usize, Option<DbId>, Option<DbId>)>,
    /// Track index of each content ID.
    by_id: HashMap<DbId, usize>,
    /// Lower-case search text of each track (the searched fields, one per line); built on the
    /// first search.
    haystack: Vec<String>,
    /// Lower-case sort keys of a text column; built on the first sort by it.
    text_keys: HashMap<Col, Vec<String>>,
    /// Thumbnail URI of each track (`None` without an artwork file); looked up when its row is
    /// first shown.
    artwork: Vec<Option<Option<Arc<str>>>>,
    /// Track indices of each smart list shown so far.
    smart: HashMap<DbId, SmartRows>,
}

/// A smart list's tracks and what they were evaluated for: its rules and the local day ("in the
/// last N days" moves at midnight).
struct SmartRows {
    xml: String,
    day: i64,
    rows: Vec<usize>,
}

/// The artwork thumbnails the table has shown, by URI, with the pass they were last shown in.
/// egui keeps every image it loads (file bytes, decoded pixels and the texture) until it is told
/// to forget it, so the least recently shown are forgotten once more than [`THUMBS_KEPT`] are kept.
#[derive(Default)]
struct Thumbs {
    shown: HashMap<Arc<str>, u64>,
}

/// Thumbnails kept loaded: several screens of rows (80 px `_s` files: about 25 kB decoded each).
const THUMBS_KEPT: usize = 256;

impl Thumbs {
    fn touch(&mut self, uri: &Arc<str>, pass: u64) {
        match self.shown.get_mut(&**uri) {
            Some(last) => *last = pass,
            None => {
                self.shown.insert(uri.clone(), pass);
            }
        }
    }

    /// Forgets the least recently shown thumbnails (never one shown in this pass) down to three
    /// quarters of [`THUMBS_KEPT`], so this runs once per many new rows, not on every one.
    fn trim(&mut self, ctx: &egui::Context, pass: u64) {
        if self.shown.len() <= THUMBS_KEPT {
            return;
        }
        let mut old: Vec<(u64, Arc<str>)> = self.shown.iter().filter(|&(_, &last)| last < pass).map(|(uri, &last)| (last, uri.clone())).collect();
        old.sort_unstable_by_key(|&(last, _)| last);
        let excess = self.shown.len() - THUMBS_KEPT * 3 / 4;
        for (_, uri) in old.into_iter().take(excess) {
            self.shown.remove(&uri);
            ctx.forget_image(&uri);
        }
    }
}

impl Lookups {
    fn follow(&mut self, library: &LibrarySnapshot) {
        let id = |t: Option<&mb_core::Track>| t.map(|t| t.id.clone());
        let key = Some((std::ptr::from_ref(library) as usize, library.tracks.len(), id(library.tracks.first()), id(library.tracks.last())));
        if self.built_for != key {
            *self = Lookups {
                built_for: key,
                by_id: library.tracks.iter().enumerate().map(|(i, t)| (t.id.clone(), i)).collect(),
                artwork: vec![None; library.tracks.len()],
                ..Lookups::default()
            };
        }
    }

    fn haystack(&mut self, library: &LibrarySnapshot) -> &[String] {
        if self.haystack.len() != library.tracks.len() {
            self.haystack = library
                .tracks
                .iter()
                .map(|t| [&t.title, &t.artist, &t.album, &t.genre, &t.label, &t.comment, &t.remixer].map(|f| f.as_str()).join("\n").to_lowercase())
                .collect();
        }
        &self.haystack
    }

    fn text_keys(&mut self, library: &LibrarySnapshot, col: Col) -> &[String] {
        self.text_keys.entry(col).or_insert_with(|| {
            library
                .tracks
                .iter()
                .map(|t| match col {
                    Col::Title => t.title.to_lowercase(),
                    Col::Artist => t.artist.to_lowercase(),
                    Col::Album => t.album.to_lowercase(),
                    _ => t.genre.to_lowercase(),
                })
                .collect()
        })
    }

    /// The `file://` URI of track `index`'s thumbnail: the 80 px `_s` file next to its artwork
    /// (docs/re/masterdb.md, `ImagePath`) when there is one, else the artwork itself (device
    /// exports keep only a small one). The files are looked up once per snapshot.
    fn thumbnail_uri(&mut self, index: usize, path: Option<&Path>) -> Option<&Arc<str>> {
        let slot = self.artwork.get_mut(index)?;
        slot.get_or_insert_with(|| {
            let path = path?;
            let small = path.file_stem().and_then(|s| s.to_str()).map(|stem| path.with_file_name(format!("{stem}_s.jpg")));
            let chosen = small.filter(|p| p.is_file()).or_else(|| path.is_file().then(|| path.to_path_buf()))?;
            Some(Arc::from(format!("file://{}", chosen.display())))
        })
        .as_ref()
    }

    /// Track indices of the smart list `id`. rekordbox stores no entries for smart lists, so they
    /// are evaluated here, once per snapshot, rules and day rather than on every keystroke or sort.
    fn smart_rows(&mut self, library: &LibrarySnapshot, id: &DbId) -> &[usize] {
        let xml = library.smart_lists.get(id).map_or("", String::as_str);
        let clock = crate::smartlist_ui::local_clock();
        let day = (clock.unix + clock.utc_offset).div_euclid(86_400);
        if !self.smart.get(id).is_some_and(|s| s.xml == xml && s.day == day) {
            let by_id = &self.by_id;
            let rows = crate::smartlist_ui::evaluate(library, id).unwrap_or_default().iter().filter_map(|id| by_id.get(id).copied()).collect();
            self.smart.insert(id.clone(), SmartRows { xml: xml.to_owned(), day, rows });
        }
        self.smart.get(id).map_or(&[], |s| s.rows.as_slice())
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            search: String::new(),
            sort: Col::Index,
            ascending: true,
            key_notation: KeyNotation::Original,
            rows: Vec::new(),
            built_for: None,
            selected: None,
            selection: BTreeSet::new(),
            anchor: None,
            entry_pos: Vec::new(),
            reveal_selected: false,
            scroll_to_top: false,
            lookups: Lookups::default(),
            thumbs: Thumbs::default(),
        }
    }
}

impl State {
    pub fn invalidate(&mut self) {
        self.built_for = None;
        self.lookups = Lookups::default();
    }

    pub fn visible_count(&self) -> usize {
        self.rows.len()
    }

    fn rebuild(&mut self, library: &LibrarySnapshot, source: &Source) {
        let snapshot = std::ptr::from_ref(library) as usize;
        let key = (source.clone(), self.search.clone(), self.sort, self.ascending, snapshot, library.tracks.len());
        if self.built_for.as_ref() == Some(&key) {
            return;
        }
        self.lookups.follow(library);
        let same_list = self.built_for.as_ref().is_some_and(|(s, .., ptr, _)| s == source && *ptr == snapshot);
        self.reveal_selected = same_list && self.selected.is_some_and(|(l, _)| l == source.library);
        self.scroll_to_top = self.built_for.as_ref().is_some_and(|(s, ..)| s != source);
        if !same_list {
            self.selection.clear();
            self.anchor = None;
            if self.selected.is_some_and(|(l, _)| l == source.library) {
                self.selected = None;
            }
        }
        // (track index, position in the list's entries)
        let mut rows: Vec<(usize, Option<usize>)> = match &source.list {
            List::Collection => (0..library.tracks.len()).map(|i| (i, None)).collect(),
            // Smart list rows have no entry position, so entry edits do not apply.
            List::Playlist(id) if library.smart_lists.contains_key(id) => {
                self.lookups.smart_rows(library, id).iter().map(|&i| (i, None)).collect()
            }
            List::Playlist(id) | List::History(id) | List::MyTag(id) => {
                let entries = match &source.list {
                    List::Playlist(_) => &library.playlist_entries,
                    List::History(_) => &library.history_entries,
                    _ => &library.my_tag_entries,
                };
                let by_id = &self.lookups.by_id;
                entries
                    .get(id)
                    .map(|ids| {
                        ids.iter()
                            .enumerate()
                            .filter_map(|(pos, id)| by_id.get(id).map(|&i| (i, Some(pos))))
                            .collect()
                    })
                    .unwrap_or_default()
            }
        };
        let needle = self.search.to_lowercase();
        if !needle.is_empty() {
            let haystack = self.lookups.haystack(library);
            rows.retain(|&(i, _)| haystack[i].contains(&needle));
        }
        let tracks = &library.tracks;
        match self.sort {
            Col::Index => {}
            Col::Title | Col::Artist | Col::Album | Col::Genre => {
                let keys = self.lookups.text_keys(library, self.sort);
                rows.sort_by(|&(a, _), &(b, _)| keys[a].cmp(&keys[b]));
            }
            Col::Key => rows.sort_by_cached_key(|&(i, _)| wheel_index(&tracks[i].key)),
            col => rows.sort_by(|&(a, _), &(b, _)| compare(&tracks[a], &tracks[b], col)),
        }
        if !self.ascending {
            rows.reverse();
        }
        self.entry_pos = rows.iter().map(|&(_, p)| p).collect();
        self.rows = rows.into_iter().map(|(i, _)| i).collect();
        self.built_for = Some(key);
    }
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
        Col::Artwork => a.artwork_path.is_some().cmp(&b.artwork_path.is_some()),
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
        Col::Rating | Col::Color | Col::Artwork => String::new(),
        Col::DateAdded => t.date_added.clone(),
        Col::Bitrate => t.bitrate.map(|b| format!("{b} kbps")).unwrap_or_default(),
        Col::Kind => t.file_type.label().to_owned(),
    }
}

/// Context for write actions offered by the table.
pub struct EditCtx<'a> {
    pub enabled: bool,
    /// The local library's playlists, for "Add to playlist".
    pub playlists: &'a [PlaylistNode],
}

/// What the table lists, for the toolbar: the collection or the list's name.
fn list_title(library: &LibrarySnapshot, source: &Source) -> String {
    let name = |nodes: &[PlaylistNode], id: &DbId| nodes.iter().find(|n| &n.id == id).map(|n| n.name.clone());
    match &source.list {
        List::Collection if source.library == LibraryRef::Local => Some("Collection".to_owned()),
        List::Collection => Some("All Tracks".to_owned()),
        List::Playlist(id) => name(&library.playlists, id),
        List::History(id) => name(&library.histories, id),
        List::MyTag(id) => name(&library.my_tags, id),
    }
    .unwrap_or_default()
}

/// The bar above the table: list name and count on the left, search on the right.
fn toolbar(ui: &mut egui::Ui, library: &LibrarySnapshot, source: &Source, state: &mut State) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::BG_DECK);
    // Painted, not labels: the tree row already carries the list's name for accessibility.
    let painter = ui.painter();
    let title = painter.layout_no_wrap(list_title(library, source), theme::semibold(15.0), theme::TEXT_STRONG);
    let count = state.rows.len();
    let count = painter.layout_no_wrap(format!("{count} {}", if count == 1 { "track" } else { "tracks" }), egui::FontId::proportional(13.0), theme::TEXT_MUTED);
    let left = rect.left() + 14.0;
    let baseline_gap = title.size().y - count.size().y;
    painter.galley(egui::pos2(left, rect.center().y - title.size().y / 2.0), title.clone(), theme::TEXT_STRONG);
    painter.galley(egui::pos2(left + title.size().x + 8.0, rect.center().y - title.size().y / 2.0 + baseline_gap * 0.8), count, theme::TEXT_MUTED);
    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(14.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| search_field(ui, &mut state.search));
}

/// Rounded search field with a magnifier, and a clear button while it has text.
fn search_field(ui: &mut egui::Ui, search: &mut String) {
    let width = 260.0_f32.min(ui.available_width());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::hover());
    let focused_id = ui.id().with("search");
    let focused = ui.memory(|m| m.has_focus(focused_id));
    ui.painter().rect(
        rect,
        14.0,
        theme::BG_WELL,
        egui::Stroke::new(1.0, if focused { theme::ACCENT } else { theme::LINE }),
        egui::StrokeKind::Inside,
    );
    icons::paint(ui.painter(), egui::Rect::from_center_size(egui::pos2(rect.left() + 16.0, rect.center().y), egui::vec2(13.0, 13.0)), Icon::Search, theme::TEXT_MUTED);
    let clear = !search.is_empty();
    let text_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + 28.0, rect.top()), egui::pos2(rect.right() - if clear { 28.0 } else { 12.0 }, rect.bottom()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.add(
        egui::TextEdit::singleline(search)
            .id(focused_id)
            .hint_text("Search")
            .frame(egui::Frame::NONE)
            .background_color(egui::Color32::TRANSPARENT)
            .desired_width(text_rect.width())
            .vertical_align(egui::Align::Center),
    );
    if clear {
        let button = egui::Rect::from_center_size(egui::pos2(rect.right() - 15.0, rect.center().y), egui::vec2(20.0, 20.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(button));
        if icons::button(&mut child, Icon::Close, 20.0, theme::TEXT_DIM, "Clear search").clicked() {
            search.clear();
        }
    }
}

pub fn show(ui: &mut egui::Ui, library: &LibrarySnapshot, source: &Source, state: &mut State, edit: &EditCtx) -> Vec<Action> {
    let mut actions = Vec::new();
    let editable = edit.enabled && source.library == LibraryRef::Local;
    state.rebuild(library, source);
    toolbar(ui, library, source, state);
    state.rebuild(library, source);
    ui.spacing_mut().item_spacing = egui::vec2(COLUMN_GAP, 0.0);
    // Column edges show only while hovered (resize handle); rows hover in a quiet tint.
    ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
    ui.visuals_mut().widgets.hovered.bg_fill = theme::ROW_HOVER;
    ui.visuals_mut().widgets.hovered.bg_stroke = egui::Stroke::new(1.0, theme::LINE);

    let row_height = theme::ROW_HEIGHT;
    let header_height = 28.0;
    let header_top = ui.cursor().top();
    ui.painter().rect_filled(
        egui::Rect::from_min_size(egui::pos2(ui.max_rect().left(), header_top), egui::vec2(ui.max_rect().width(), header_height)),
        0.0,
        theme::BG_BAR,
    );
    ui.painter().hline(ui.max_rect().x_range(), header_top + header_height - 0.5, egui::Stroke::new(1.0, theme::SEAM));
    // The title fills what the other columns leave. Artist, album and genre start narrower when
    // the browser is narrow (first layout only; egui remembers widths the user drags).
    let spacing = COLUMN_GAP * (COLUMNS.len() - 1) as f32;
    let fixed: f32 = COLUMNS.iter().filter(|(c, ..)| !c.shrinks() && *c != Col::Title).map(|(_, _, w)| w).sum();
    let flexible: f32 = COLUMNS.iter().filter(|(c, ..)| c.shrinks()).map(|(_, _, w)| w).sum();
    let room = ui.available_width() - fixed - spacing - 12.0 - TITLE_COMFORT;
    let scale = (room / flexible).clamp(0.6, 1.0);
    let pass = ui.ctx().cumulative_pass_nr();
    let reveal = std::mem::take(&mut state.reveal_selected)
        .then(|| state.selected.and_then(|(_, i)| state.rows.iter().position(|&r| r == i)))
        .flatten();
    let mut table = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click_and_drag())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .min_scrolled_height(0.0)
        .auto_shrink(false);
    if let Some(row) = reveal {
        table = table.scroll_to_row(row, Some(egui::Align::Center));
    } else if std::mem::take(&mut state.scroll_to_top) {
        table = table.vertical_scroll_offset(0.0);
    }
    for (col, _, width) in COLUMNS {
        table = table.column(match col {
            Col::Title => Column::remainder().at_least(140.0).clip(true),
            c if c.painted() => Column::exact(*width),
            c if c.shrinks() => Column::initial((width * scale).round()).at_least(24.0).clip(true),
            _ => Column::initial(*width).at_least(24.0).clip(true),
        });
    }
    let mut clicked_header = None;
    table
        .header(header_height, |mut header| {
            for (col, label, _) in COLUMNS {
                header.col(|ui| {
                    let edge = ui.max_rect().right() + 5.0;
                    let y = ui.max_rect().center().y;
                    if !col.painted() {
                        ui.painter().vline(edge, (y - 7.0)..=(y + 7.0), egui::Stroke::new(1.0, theme::LINE));
                    }
                    let layout = if col.right_aligned() { egui::Layout::right_to_left(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) };
                    ui.with_layout(layout, |ui| {
                        let sorted = state.sort == *col && *col != Col::Index;
                        let color = if sorted { theme::TEXT_STRONG } else { theme::TEXT_HEADER };
                        let text = egui::RichText::new(*label).font(theme::medium(11.5)).color(color);
                        if ui.add(egui::Label::new(text).sense(egui::Sense::click()).selectable(false)).clicked() {
                            clicked_header = Some(*col);
                        }
                        if sorted {
                            sort_arrow(ui, state.ascending);
                        }
                    });
                });
            }
        })
        .body(|body| {
            body.rows(row_height, state.rows.len(), |mut row| {
                let display_index = row.index();
                let track_index = state.rows[display_index];
                let track = &library.tracks[track_index];
                let is_selected = state.selection.contains(&track_index);
                row.set_selected(is_selected);
                for (col, _, _) in COLUMNS {
                    row.col(|ui| match col {
                        Col::Rating => stars(ui, track.rating),
                        Col::Color => color_bar(ui, track.color),
                        Col::Artwork => {
                            let uri = state.lookups.thumbnail_uri(track_index, track.artwork_path.as_deref());
                            if let Some(uri) = uri {
                                state.thumbs.touch(uri, pass);
                            }
                            thumbnail(ui, uri.map(|u| &**u));
                        }
                        _ => {
                            let color = match col {
                                _ if is_selected => theme::TEXT_STRONG,
                                Col::Title => theme::TEXT,
                                Col::Key => theme::key_color(&track.key),
                                Col::Index => theme::TEXT_MUTED,
                                _ => theme::TEXT_DIM,
                            };
                            let text = egui::RichText::new(cell_text(track, *col, display_index, state.key_notation)).color(color);
                            let label = egui::Label::new(text).truncate().selectable(false);
                            if col.right_aligned() {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.add(label));
                            } else {
                                ui.add(label);
                            }
                        }
                    });
                }
                let response = row.response();
                if response.clicked() || (response.secondary_clicked() && !is_selected) {
                    let mods = response.ctx.input(|i| i.modifiers);
                    click_row(state, display_index, track_index, mods, response.secondary_clicked());
                    state.selected = Some((source.library, track_index));
                }
                if editable && response.drag_started() {
                    if !is_selected {
                        click_row(state, display_index, track_index, egui::Modifiers::NONE, false);
                    }
                    let ids = state.selection.iter().map(|&i| library.tracks[i].id.clone()).collect();
                    egui::DragAndDrop::set_payload(&response.ctx, TrackDrag(ids));
                }
                // The menu is drawn every frame while it is open: what it acts on is collected
                // only when an item is clicked.
                response.context_menu(|ui| {
                    if editable {
                        row_menu(ui, library, source, state, edit, &mut actions);
                    } else if let LibraryRef::Device(device) = source.library {
                        let tracks = || state.selection.iter().map(|&i| library.tracks[i].path.clone()).collect::<Vec<_>>();
                        ui.weak(format!("{} track(s)", state.selection.len()));
                        if ui.button("Import to Collection").clicked() {
                            actions.push(Action::ImportFromDevice { device, playlist: None, tracks: tracks() });
                            ui.close();
                        }
                        if ui.button("Remove from device…").clicked() {
                            actions.push(Action::AskRemoveFromDevice { device, playlists: Vec::new(), tracks: tracks() });
                            ui.close();
                        }
                    } else if source.library == LibraryRef::Xml {
                        ui.weak(format!("{} track(s)", state.selection.len()));
                        if ui.button("Import to Collection").clicked() {
                            actions.push(Action::ImportFromXml { list: None, tracks: state.selection.iter().copied().collect() });
                            ui.close();
                        }
                    } else {
                        ui.weak("Turn on editing to change the library");
                    }
                });
            });
        });
    state.thumbs.trim(ui.ctx(), pass);
    if state.rows.is_empty() {
        let message =if state.search.is_empty() { "No tracks".to_owned() } else { format!("No tracks match \u{201c}{}\u{201d}", state.search) };
        let rect = egui::Rect::from_min_size(egui::pos2(ui.max_rect().left(), header_top + header_height + 40.0), egui::vec2(ui.max_rect().width(), 20.0));
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, message, egui::FontId::proportional(13.0), theme::TEXT_MUTED);
    }
    if let Some(col) = clicked_header.filter(|c| !c.painted()) {
        if state.sort == col {
            state.ascending = !state.ascending;
        } else {
            state.sort = col;
            state.ascending = true;
        }
    }
    if let Some(drag) = egui::DragAndDrop::payload::<TrackDrag>(ui.ctx()) {
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::new("track-drag"), egui::PopupAnchor::Pointer)
            .show(|ui| ui.label(format!("{} track(s)", drag.0.len())));
    }
    actions
}

/// Plain click selects one row, ⌘ toggles, ⇧ extends from the anchor (display order).
fn click_row(state: &mut State, display_index: usize, track_index: usize, mods: egui::Modifiers, secondary: bool) {
    if mods.command && !secondary {
        if !state.selection.remove(&track_index) {
            state.selection.insert(track_index);
        }
        state.anchor = Some(display_index);
    } else if mods.shift && !secondary {
        let anchor = state.anchor.unwrap_or(display_index);
        let (lo, hi) = (anchor.min(display_index), anchor.max(display_index));
        state.selection = state.rows[lo..=hi].iter().copied().collect();
    } else {
        state.selection = BTreeSet::from([track_index]);
        state.anchor = Some(display_index);
    }
}

fn selected_ids(state: &State, library: &LibrarySnapshot) -> Vec<DbId> {
    state.selection.iter().map(|&i| library.tracks[i].id.clone()).collect()
}

/// Write actions for the selected rows (local library, editing on). Drawn every frame while the
/// menu is open, so the selection's IDs are collected only for a clicked item.
fn row_menu(ui: &mut egui::Ui, library: &LibrarySnapshot, source: &Source, state: &State, edit: &EditCtx, actions: &mut Vec<Action>) {
    let ids = || selected_ids(state, library);
    ui.weak(format!("{} track(s)", state.selection.len()));
    ui.menu_button("Add to playlist", |ui| {
        let mut any = false;
        for p in edit.playlists.iter().filter(|p| p.kind == PlaylistKind::Playlist) {
            any = true;
            if ui.button(&p.name).clicked() {
                actions.push(Action::Run(Command::AddTracks { playlist: p.id.clone(), content_ids: ids(), position: None }));
                ui.close();
            }
        }
        if !any {
            ui.weak("No playlists yet");
        }
    });
    if let List::Playlist(playlist) = &source.list
        && let Some(entry_ids) = library.playlist_entry_ids.get(playlist)
        && ui.button("Remove from playlist").clicked()
    {
        {
            let entries: Vec<String> = state
                .rows
                .iter()
                .zip(&state.entry_pos)
                .filter(|(i, _)| state.selection.contains(i))
                .filter_map(|(_, pos)| pos.and_then(|p| entry_ids.get(p).cloned()))
                .collect();
            actions.push(Action::Run(Command::RemoveFromPlaylist { playlist: playlist.clone(), entry_ids: entries }));
            ui.close();
        }
    }
    ui.menu_button("Rating", |ui| {
        for stars in 0..=5u8 {
            let label = if stars == 0 { "None".to_owned() } else { "★".repeat(stars as usize).replace('★', "*") };
            if ui.button(label).clicked() {
                for id in ids() {
                    actions.push(Action::Run(Command::EditTrack { id, edit: TrackEdit::Rating(stars) }));
                }
                ui.close();
            }
        }
    });
    ui.menu_button("Colour", |ui| {
        for (slot, name) in [(0u8, "None"), (1, "Pink"), (2, "Red"), (3, "Orange"), (4, "Yellow"), (5, "Green"), (6, "Aqua"), (7, "Blue"), (8, "Purple")] {
            if ui.button(name).clicked() {
                for id in ids() {
                    actions.push(Action::Run(Command::EditTrack { id, edit: TrackEdit::Color(slot) }));
                }
                ui.close();
            }
        }
    });
    if ui.button("Analyze Track").clicked() {
        actions.push(Action::AnalyzeTracks(ids()));
        ui.close();
    }
    // rekordbox's "Analysis Lock On/Off": locked tracks are not analysed and their grid stays.
    // The selection holds track indices, so this reads the selected tracks directly.
    let locked = |want: bool| state.selection.iter().any(|&i| library.tracks.get(i).is_some_and(|t| (t.analysed & 0x80 != 0) == want));
    if locked(false) && ui.button("Analysis Lock On").clicked() {
        actions.push(Action::Run(Command::SetAnalysisLock { ids: ids(), locked: true }));
        ui.close();
    }
    if locked(true) && ui.button("Analysis Lock Off").clicked() {
        actions.push(Action::Run(Command::SetAnalysisLock { ids: ids(), locked: false }));
        ui.close();
    }
    ui.separator();
    if ui.button("Remove from Collection…").clicked() {
        actions.push(Action::AskRemoveFromCollection(ids()));
        ui.close();
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

/// rekordbox track colour slot (1 Pink … 8 Purple, `djmdColor`) as a bar at the row's start.
fn color_bar(ui: &mut egui::Ui, color: Option<u8>) {
    let rect = ui.max_rect();
    ui.allocate_exact_size(egui::vec2(rect.width(), 1.0), egui::Sense::hover());
    if let Some(c) = color.and_then(theme::track_color) {
        let bar = egui::Rect::from_center_size(rect.center(), egui::vec2(4.0, rect.height() - 6.0));
        ui.painter().rect_filled(bar, 2.0, c);
    }
}

/// The track's small artwork ([`Lookups::thumbnail_uri`]) when there is a file, else nothing.
fn thumbnail(ui: &mut egui::Ui, uri: Option<&str>) {
    let size = egui::vec2(18.0, 18.0);
    match uri {
        Some(uri) => {
            ui.add(egui::Image::new(uri).fit_to_exact_size(size).corner_radius(2.0).show_loading_spinner(false));
        }
        None => {
            ui.allocate_exact_size(size, egui::Sense::hover());
        }
    }
}
