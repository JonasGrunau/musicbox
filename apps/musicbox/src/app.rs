use crate::actions::Action;
use crate::devices::{DeviceState, Devices};
use crate::editor::{Editor, Reply, Request};
use crate::device_remove::{self, RemoveJob};
use crate::export::{self, ExportJob};
use crate::library::{LoadResult, spawn_load};
use crate::{Args, browser, cue_panel, deck, icons, theme, tree, waveform};
use mb_library::{Command, NodeKind};
use mb_core::{DbId, LibrarySnapshot};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

/// Which library a list belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibraryRef {
    Local,
    /// Index into `Devices::list`.
    Device(usize),
    /// The opened rekordbox.xml ("rekordbox xml" in the tree).
    Xml,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum List {
    Collection,
    Playlist(DbId),
    History(DbId),
    MyTag(DbId),
}

/// What the track table currently lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub library: LibraryRef,
    pub list: List,
}

/// Commands of the native menu bar (`menu_bar.rs`); the headless tests send them directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommand {
    OpenXml,
    CloseXml,
    ExportXml,
    Undo,
    Redo,
    Quit,
}

/// Modal dialogs that collect input before a command runs.
enum Dialog {
    Create { parent: Option<DbId>, kind: NodeKind, name: String },
    Rename { id: DbId, name: String },
    ConfirmRemove { ids: Vec<DbId> },
    /// Remove lists (name paths) and tracks (device paths) from a device.
    ConfirmDeviceRemove { device: usize, playlists: Vec<Vec<String>>, tracks: Vec<String>, orphans: bool },
    /// The intelligent playlist editor.
    Smart(Box<crate::smartlist_ui::SmartEditor>),
    /// "Export Collection in xml format": the file to write.
    XmlExport { path: String },
    /// "Open rekordbox xml": the file to show.
    XmlOpen { path: String },
    /// rekordbox's two questions before an XML import: update tracks already in the collection,
    /// replace lists with the same name.
    ConfirmXmlImport { import: Box<mb_library::XmlImport>, existing: usize, same_name: Vec<String> },
    /// A device's My Settings.
    MySettings(Box<crate::mysettings_ui::MySettingsDialog>),
}

pub struct App {
    args: Args,
    loading: Option<Receiver<LoadResult>>,
    error: Option<String>,
    pub library: LibrarySnapshot,
    /// master.db path and passphrase of the loaded local library (for the editor).
    local_db: Option<(PathBuf, String)>,
    /// Source-library details for USB exports.
    export_source: Option<export::Source>,
    export: Option<ExportJob>,
    remove: Option<RemoveJob>,
    editor: Option<Editor>,
    dialog: Option<Dialog>,
    /// Last editor message (status bar).
    edit_status: Option<String>,
    last_guard_check: Option<std::time::Instant>,
    /// The running background check for rekordbox processes.
    guard_check: Option<Receiver<Vec<String>>>,
    ctx: egui::Context,
    pub devices: Devices,
    pub source: Source,
    pub browser: browser::State,
    waves: HashMap<PathBuf, Option<waveform::TrackWaves>>,
    /// Zoomed waveforms built from the audio of the selected tracks.
    zoom_views: waveform::ZoomViews,
    /// Preview player for one track (path of the open file).
    player: Option<(PathBuf, mb_player::Player)>,
    wave_style: waveform::Style,
    /// Cues of the local library by content ID (`djmdCue`, loaded with the snapshot).
    cues: HashMap<DbId, Vec<mb_library::Cue>>,
    deck_edit: cue_panel::State,
    /// Local track to select again once a reloaded snapshot is shown (edits keep the deck).
    reselect: Option<DbId>,
    /// Device of the running "Import to Collection" / "Import Histories" (root, may write).
    device_import: Option<(PathBuf, bool)>,
    /// What the stick-side part of the last device import did (joined into the status line).
    device_import_note: Option<String>,
    /// Why the last analysis skipped a pass (vocal), shown with the next status message.
    analysis_note: Option<String>,
    /// The opened rekordbox.xml and its loader / the running XML export.
    xml: Option<crate::xml_bridge::XmlLibrary>,
    xml_loading: Option<Receiver<Result<crate::xml_bridge::XmlLibrary, String>>>,
    xml_export: Option<crate::xml_bridge::XmlExportJob>,
    /// musicbox's own analysis: the worker queue (started on first use), the preferences it runs
    /// with, and the tracks already queued automatically this session.
    analysis: Option<crate::analysis_jobs::AnalysisQueue>,
    analysis_settings: mb_library::AnalysisSettings,
    /// Preference `AutoAnalysis`: analyse tracks that have no analysis yet (after an import).
    auto_analysis: bool,
    auto_queued: std::collections::HashSet<DbId>,
    /// The native menu bar, when the app runs in a window (not in the headless tests).
    #[cfg(target_os = "macos")]
    menu_bar: Option<crate::menu_bar::MenuBar>,
    /// Menu commands waiting for this frame.
    menu_commands: Vec<MenuCommand>,
    /// The title bar the top bar covers (`window_chrome`).
    title_bar: crate::window_chrome::TitleBar,
}

static EMPTY: std::sync::LazyLock<LibrarySnapshot> = std::sync::LazyLock::new(LibrarySnapshot::default);

impl App {
    pub fn new(ctx: egui::Context, args: Args) -> Self {
        let devices = Devices::new(args.devices.clone(), !args.no_volume_scan);
        let ui_ctx = ctx.clone();
        Self {
            loading: Some(spawn_load(ctx, args.clone())),
            args,
            error: None,
            library: LibrarySnapshot::default(),
            local_db: None,
            export_source: None,
            export: None,
            remove: None,
            device_import: None,
            device_import_note: None,
            analysis_note: None,
            xml: None,
            xml_loading: None,
            xml_export: None,
            editor: None,
            dialog: None,
            edit_status: None,
            last_guard_check: None,
            guard_check: None,
            ctx: ui_ctx,
            devices,
            source: Source { library: LibraryRef::Local, list: List::Collection },
            browser: browser::State::default(),
            waves: HashMap::new(),
            zoom_views: waveform::ZoomViews::default(),
            player: None,
            wave_style: waveform::Style::Rgb,
            cues: HashMap::new(),
            deck_edit: cue_panel::State::default(),
            reselect: None,
            analysis: None,
            analysis_settings: mb_library::AnalysisSettings::default(),
            auto_analysis: false,
            auto_queued: std::collections::HashSet::new(),
            #[cfg(target_os = "macos")]
            menu_bar: None,
            menu_commands: Vec::new(),
            title_bar: crate::window_chrome::TitleBar::default(),
        }
    }

    /// The window's title bar layout, as `window_chrome::install` read it.
    pub fn set_title_bar(&mut self, title_bar: crate::window_chrome::TitleBar) {
        self.title_bar = title_bar;
    }

    /// Installs the native menu bar (macOS; main thread). Without it the File, Edit and app
    /// commands are reachable through [`App::menu_command`] only.
    pub fn install_menu_bar(&mut self) {
        #[cfg(target_os = "macos")]
        match crate::menu_bar::MenuBar::install(&self.ctx) {
            Ok(bar) => self.menu_bar = Some(bar),
            Err(e) => self.edit_status = Some(format!("No menu bar: {e}")),
        }
    }

    /// Runs a menu bar command (in the next frame).
    pub fn menu_command(&mut self, command: MenuCommand) {
        self.menu_commands.push(command);
        self.ctx.request_repaint();
    }

    fn run_menu_commands(&mut self, ctx: &egui::Context) -> Vec<Action> {
        #[cfg(target_os = "macos")]
        {
            let enabled = [true, self.xml.is_some(), self.export_source.is_some(), self.editing()];
            if let Some(bar) = &mut self.menu_bar {
                self.menu_commands.extend(bar.poll());
                bar.sync(enabled);
            }
        }
        let mut actions = Vec::new();
        for command in std::mem::take(&mut self.menu_commands) {
            match command {
                MenuCommand::OpenXml => {
                    let path = self.xml.as_ref().map(|x| x.path.display().to_string()).unwrap_or_default();
                    self.dialog = Some(Dialog::XmlOpen { path });
                }
                MenuCommand::CloseXml => {
                    self.xml = None;
                    if self.source.library == LibraryRef::Xml {
                        self.source = Source { library: LibraryRef::Local, list: List::Collection };
                    }
                }
                MenuCommand::ExportXml if self.export_source.is_some() => {
                    self.dialog = Some(Dialog::XmlExport { path: crate::xml_bridge::default_export_path() });
                }
                MenuCommand::ExportXml => {}
                MenuCommand::Undo => actions.push(Action::Undo),
                MenuCommand::Redo => actions.push(Action::Redo),
                MenuCommand::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        actions
    }

    pub fn snapshot(&self, library: LibraryRef) -> &LibrarySnapshot {
        match library {
            LibraryRef::Local => &self.library,
            LibraryRef::Device(i) => self.devices.list.get(i).and_then(|d| d.snapshot()).unwrap_or(&EMPTY),
            LibraryRef::Xml => self.xml.as_ref().map_or(&*EMPTY, |x| &x.snapshot),
        }
    }

    fn poll_loading(&mut self) {
        let Some(rx) = &self.loading else { return };
        if let Ok(result) = rx.try_recv() {
            self.loading = None;
            match result {
                Ok(loaded) => {
                    self.reselect = self
                        .browser
                        .selected
                        .filter(|(library, _)| *library == LibraryRef::Local)
                        .and_then(|(_, i)| self.library.tracks.get(i))
                        .map(|t| t.id.clone());
                    self.library = loaded.snapshot;
                    self.cues = loaded.cues;
                    // Key column like rekordbox: preferences ShowOriginalKey / KeyStringSetting.
                    let pref = |name| mb_usb::settings::preference(&loaded.settings_root, name);
                    self.browser.key_notation = mb_core::key::KeyNotation::from_preferences(
                        pref("ShowOriginalKey").map(|v| v.trim() != "0"),
                        pref("KeyStringSetting").and_then(|v| v.trim().parse().ok()),
                    );
                    // Analysis preferences: `AutoGain` and `enableKeyAnalysis` (default on),
                    // `trackAnalysisSettingsSongStruct`, `trackAnalysisMode`, `HighPrecisionBeatAnalyze` with
                    // `trackHiPrecisionBeatAnalysisMode`, `trackAnalysisSettingsVocalDetect`,
                    // `TrackAnalysisSettingsEmbedding` (default on), the CUE analysis preferences
                    // (`DetectCue`, `AutoDetectCue`, …) and `cue_personal_trend.json`, `AutoAnalysis`.
                    self.analysis_settings = mb_library::AnalysisSettings {
                        auto_gain: pref("AutoGain").is_none_or(|v| v.trim() != "0"),
                        phrase: pref("trackAnalysisSettingsSongStruct").is_some_and(|v| v.trim() == "1"),
                        key: pref("enableKeyAnalysis").is_none_or(|v| v.trim() != "0"),
                        high_precision: mb_library::AnalysisSettings::high_precision_from(
                            pref("HighPrecisionBeatAnalyze").as_deref(),
                            pref("trackHiPrecisionBeatAnalysisMode").as_deref(),
                        ),
                        dynamic: mb_library::AnalysisSettings::dynamic_from(pref("trackAnalysisMode").as_deref()),
                        vocal: pref("trackAnalysisSettingsVocalDetect").is_some_and(|v| v.trim() == "1"),
                        embedding: pref("TrackAnalysisSettingsEmbedding").is_none_or(|v| v.trim() != "0"),
                        cue: mb_library::CueAnalysisSettings::from_prefs(|n| mb_usb::settings::preference(&loaded.settings_root, n)),
                        // Auto CUE's personal tendency, read only; an unreadable file counts as none
                        cue_trend: mb_library::cue_analysis::read_trend(&loaded.settings_root).ok().flatten().map(std::sync::Arc::new),
                    };
                    self.auto_analysis = pref("AutoAnalysis").is_some_and(|v| v.trim() == "1");
                    self.export_source = Some(export::Source {
                        db_path: loaded.db_path.clone(),
                        passphrase: loaded.passphrase.clone(),
                        analysis_root: loaded.analysis_root,
                        settings_root: loaded.settings_root,
                        lang_path: loaded.lang_path,
                    });
                    if self.xml.is_none()
                        && let Some(path) = self.export_source.as_ref().and_then(|s| crate::xml_bridge::bridge_file(&s.settings_root))
                    {
                        self.open_xml(path);
                    }
                    self.local_db = Some((loaded.db_path, loaded.passphrase));
                    self.browser.invalidate();
                    self.queue_unanalysed();
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// The player area: the zoomed waveform, artwork, title, readouts and the overview, and for
    /// collection tracks the cue and grid controls (`cue_panel`). Layout: docs/design.md § Deck.
    fn deck(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let selected = self.browser.selected;
        let track = selected.and_then(|(library, i)| self.snapshot(library).tracks.get(i)).cloned();
        let local = selected.is_some_and(|(library, _)| library == LibraryRef::Local);
        let editing = self.editing();
        if let Some(t) = &track {
            self.deck_edit.follow(&t.id, t.bpm());
        }
        let waves = track.as_ref().and_then(|t| t.analysis_path.as_ref()).and_then(|path| {
            self.waves.entry(path.clone()).or_insert_with(|| waveform::load(path)).as_ref()
        });
        // Built from the audio for tracks with analysis files (the strip needs their grid).
        if let (Some(t), Some(_)) = (&track, waves) {
            self.zoom_views.update(&t.path, ui.ctx());
        }
        let view = track.as_ref().filter(|_| waves.is_some()).and_then(|t| self.zoom_views.view(&t.path));
        let wave_style = &mut self.wave_style;
        let player = track.as_ref().and_then(|t| self.player.as_ref().filter(|(p, _)| *p == t.path)).map(|(_, pl)| pl);
        let playing = player.is_some_and(|p| p.is_playing());
        let playhead = player.map(|p| p.position_ms());
        let position = playhead.unwrap_or(self.deck_edit.cursor_ms);
        if playing {
            ui.ctx().request_repaint();
        }
        let db_cues: &[mb_library::Cue] = match &track {
            Some(t) if local => self.cues.get(&t.id).map_or(&[], Vec::as_slice),
            _ => &[],
        };
        let marks = match (local, waves) {
            (true, _) => cue_panel::marks(db_cues),
            (false, Some(w)) => w.cues.clone(),
            (false, None) => Vec::new(),
        };
        let duration = track.as_ref().map_or(0, |t| cue_panel::duration_ms(t, waves));
        let mut play_clicked = false;
        let mut seek_to = None;
        let mut jump = None;
        let mut actions = Vec::new();
        let deck_edit = &mut self.deck_edit;
        let notation = self.browser.key_notation;
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 0.0);

        match (&track, waves) {
            (Some(_), Some(w)) => jump = waveform::show_detail(ui, w, view, &marks, position, 4_000.0, deck::DETAIL_HEIGHT, *wave_style),
            (Some(_), None) => deck::empty_well(ui, deck::DETAIL_HEIGHT, "Not analysed"),
            (None, _) => deck::empty_well(ui, deck::DETAIL_HEIGHT, ""),
        }
        ui.add_space(deck::PAD);
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), deck::INFO_HEIGHT), egui::Layout::left_to_right(egui::Align::Min), |ui| {
            deck::artwork(ui, track.as_ref().and_then(|t| t.artwork_path.as_deref()), deck::INFO_HEIGHT);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 42.0), egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    deck::readouts(ui, track.as_ref(), notation, position, duration);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add_space(1.0);
                        deck::title_block(ui, track.as_ref());
                    });
                });
                ui.add_space(2.0);
                ui.horizontal_centered(|ui| {
                    play_clicked = deck::play_button(ui, playing, track.is_some()).clicked();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        deck::wave_style_switch(ui, wave_style);
                        match waves {
                            Some(w) => seek_to = waveform::show_overview(ui, w, &marks, duration, *wave_style, Some(position), 32.0),
                            None => deck::empty_well(ui, 32.0, ""),
                        }
                    });
                });
            });
        });
        if let (Some(track), true, Some(w)) = (&track, local, waves) {
            ui.add_space(deck::PAD);
            actions = cue_panel::controls(ui, track, db_cues, &w.grid, duration, position, editing, deck_edit, &mut jump);
        }
        if let Some(ms) = jump {
            // Moving the position: the open player seeks (and keeps its play state), otherwise
            // only the cursor moves; nothing starts playing.
            match track.as_ref().and_then(|t| self.player.as_ref().filter(|(p, _)| *p == t.path)) {
                Some((_, player)) if player.is_playing() => player.play_from_ms(ms),
                Some((_, player)) => player.seek_ms(ms),
                None => self.deck_edit.cursor_ms = ms,
            }
        }
        let Some(track) = track else { return actions };
        let space = ui.ctx().input(|i| i.key_pressed(egui::Key::Space)) && !ui.ctx().egui_wants_keyboard_input();
        if (play_clicked || space) && playhead.is_none() && self.deck_edit.cursor_ms > 0.0 && seek_to.is_none() {
            seek_to = Some(self.deck_edit.cursor_ms);
        }
        if play_clicked || space || seek_to.is_some() {
            self.preview(&track.path, play_clicked || space, seek_to);
        }
        actions
    }

    /// Opens the preview player for `path` if needed, then toggles playback and/or seeks.
    fn preview(&mut self, path: &std::path::Path, toggle: bool, seek_ms: Option<f64>) {
        if self.player.as_ref().is_none_or(|(p, _)| p != path) {
            match mb_player::Player::open(path) {
                Ok(player) => self.player = Some((path.to_path_buf(), player)),
                Err(e) => {
                    self.edit_status = Some(format!("Cannot play {}: {e}", path.display()));
                    self.player = None;
                    return;
                }
            }
        }
        let Some((_, player)) = &self.player else { return };
        match seek_ms {
            Some(ms) => player.play_from_ms(ms),
            None if toggle => player.toggle(),
            None => {}
        }
    }

    /// Top bar, merged with the window's title bar: the traffic lights on the left (drawn by
    /// macOS), the app mark, and the editing switch on the right. Dragging its empty part moves
    /// the window; a double click zooms it.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let bar = ui.max_rect();
        let background = ui.interact(bar, ui.id().with("title-bar"), egui::Sense::click_and_drag());
        if background.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if background.double_clicked() {
            let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
        // Room for the traffic lights, which macOS hides in full screen.
        let fullscreen = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let inset = self.title_bar.lights_end + theme::AFTER_TRAFFIC_LIGHTS - 14.0;
            ui.add_space(if fullscreen { 0.0 } else { inset });
            icons::show(ui, icons::Icon::Logo, 20.0, theme::TEXT_STRONG);
            ui.label(egui::RichText::new("musicbox").font(theme::semibold(13.5)).color(theme::TEXT_STRONG));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.editing_switch(ui));
        });
    }

    /// Status bar: the last message on the left, running jobs with their progress on the right.
    fn status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let dot = |ui: &mut egui::Ui, color: egui::Color32| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, color);
            };
            if self.loading.is_some() {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(egui::RichText::new("Loading library…").small().color(theme::TEXT_DIM));
            } else if let Some(e) = &self.error {
                dot(ui, theme::DANGER);
                ui.add(egui::Label::new(egui::RichText::new(e).small().color(theme::DANGER)).truncate());
            } else {
                dot(ui, if self.editing() { theme::ACCENT } else { theme::TEXT_MUTED });
                if self.edit_status.is_none() {
                    let count = format!("{} tracks in the collection", self.library.tracks.len());
                    ui.label(egui::RichText::new(count).small().color(theme::TEXT_DIM));
                }
            }
            // The last message stays visible next to a load error.
            if let Some(message) = &self.edit_status {
                ui.add(egui::Label::new(egui::RichText::new(message).small().color(theme::TEXT_DIM)).truncate());
            }
            if let LibraryRef::Device(i) = self.source.library
                && let Some(DeviceState::Failed(e)) = self.devices.list.get(i).map(|d| &d.state)
            {
                ui.label(egui::RichText::new(e).small().color(theme::WARN));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let job = |ui: &mut egui::Ui, text: String, fraction: Option<f32>| {
                    if let Some(f) = fraction {
                        ui.add(egui::ProgressBar::new(f).desired_width(120.0).desired_height(6.0).fill(theme::ACCENT));
                    } else {
                        ui.add(egui::Spinner::new().size(12.0));
                    }
                    ui.add(egui::Label::new(egui::RichText::new(text).small().color(theme::TEXT_DIM)).truncate());
                };
                if let Some(job_state) = &self.export {
                    if ui.small_button("Cancel").clicked() {
                        job_state.cancel();
                    }
                    let (text, fraction) = match &job_state.progress {
                        Some(p) => (
                            format!("Exporting to {}: {}/{} {}", job_state.device_name, p.tracks_done, p.tracks_total, p.current.as_deref().unwrap_or("")),
                            Some(p.tracks_done as f32 / p.tracks_total.max(1) as f32),
                        ),
                        None => (format!("Exporting to {}…", job_state.device_name), None),
                    };
                    job(ui, text, fraction);
                } else if let Some(job_state) = &self.remove {
                    job(ui, format!("Removing from {}…", job_state.device_name), None);
                } else if let Some(job_state) = &self.xml_export {
                    job(ui, format!("Exporting {}…", job_state.path.display()), None);
                }
                if let Some(p) = self.analysis.as_ref().and_then(|q| q.progress()) {
                    if ui.small_button("Cancel analysis").clicked() {
                        self.apply(vec![Action::CancelAnalysis]);
                    }
                    let text = format!("Analysing {}/{} {}", p.done, p.total, p.running.first().map(String::as_str).unwrap_or(""));
                    job(ui, text, Some(p.done as f32 / p.total.max(1) as f32));
                }
            });
        });
    }

    /// The zoomed waveform of the selected track is still being built from its audio.
    pub fn zoom_view_building(&self) -> bool {
        self.zoom_views.building()
    }

    /// The selected collection track has analysis files to edit cues and grid on.
    fn waves_ready(&mut self) -> bool {
        let Some((LibraryRef::Local, i)) = self.browser.selected else { return false };
        let Some(path) = self.library.tracks.get(i).and_then(|t| t.analysis_path.clone()) else { return false };
        self.waves.entry(path.clone()).or_insert_with(|| waveform::load(&path)).is_some()
    }

    fn editing(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.ready)
    }

    /// Off by default; turning it on opens the write connection (which backs the library up).
    fn editing_switch(&mut self, ui: &mut egui::Ui) {
        let state = match &self.editor {
            Some(e) if !e.ready => deck::EditState::Starting,
            Some(_) => deck::EditState::Editing,
            None => deck::EditState::ReadOnly,
        };
        let response = deck::editing_switch(ui, state).on_hover_text("Editing writes to the library; it backs the library up first");
        if response.clicked() {
            if self.editor.is_none() {
                let running = mb_masterdb::running_rekordbox();
                if !running.is_empty() {
                    self.edit_status = Some(format!("Quit {} first", running.join(", ")));
                } else if let Some((db, passphrase)) = &self.local_db {
                    let analysis_root = self.export_source.as_ref().map(|s| s.analysis_root.clone());
                    self.editor = Some(Editor::start(ui.ctx(), db, passphrase, self.args.backup_root.clone(), analysis_root));
                    self.edit_status = Some("Backing up the library…".into());
                }
            } else {
                self.editor = None;
                self.edit_status = Some("Editing off".into());
            }
        }
    }

    /// Editor replies; also locks editing again as soon as rekordbox starts.
    fn poll_editor(&mut self, ctx: &egui::Context) {
        // The process list is read on a worker thread (it takes tens of milliseconds), every 2 s.
        if self.editor.is_some() && self.guard_check.is_none() && self.last_guard_check.is_none_or(|t| t.elapsed().as_secs() >= 2) {
            self.last_guard_check = Some(std::time::Instant::now());
            let (tx, rx) = std::sync::mpsc::channel();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(mb_masterdb::running_rekordbox());
                repaint.request_repaint();
            });
            self.guard_check = Some(rx);
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
        if let Some(running) = self.guard_check.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.guard_check = None;
            if self.editor.is_some() && !running.is_empty() {
                self.editor = None;
                self.edit_status = Some(format!("{} started; editing turned off", running.join(", ")));
                return;
            }
        }
        let Some(editor) = &mut self.editor else { return };
        let mut reload = false;
        for reply in editor.poll() {
            match reply {
                Reply::Ready { backup } => {
                    self.edit_status = Some(format!("Editing on — backup in {}", backup.display()));
                    self.queue_unanalysed();
                }
                Reply::Failed(e) => {
                    self.edit_status = Some(e);
                    self.editor = None;
                    return;
                }
                Reply::Done { message, changed } => {
                    let notes: Vec<String> = [self.device_import_note.take(), self.analysis_note.take()].into_iter().flatten().collect();
                    self.edit_status = Some(if notes.is_empty() { message } else { format!("{message}; {}", notes.join("; ")) });
                    reload |= changed;
                }
                Reply::DeviceEffect(effect) => {
                    if let Some((root, writable)) = self.device_import.take() {
                        self.device_import_note = Some(if !writable {
                            "device is protected, nothing written to it".into()
                        } else {
                            match crate::device_import::apply_to_device(&root, &effect) {
                                Ok(Some(n)) => {
                                    self.devices.reload(ctx, &root);
                                    n
                                }
                                Ok(None) => String::new(),
                                Err(e) => format!("device not updated: {e}"),
                            }
                        })
                        .filter(|n| !n.is_empty());
                    }
                }
            }
        }
        if reload {
            // Grid edits rewrite the analysis files.
            self.waves.clear();
        }
        if reload && self.loading.is_none() {
            self.loading = Some(spawn_load(ctx.clone(), self.args.clone()));
        }
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Run(command) => self.run(command),
                Action::Undo => self.request(Request::Undo),
                Action::Redo => self.request(Request::Redo),
                Action::AskCreate { parent, kind } => {
                    let name = if kind == NodeKind::Folder { "New Folder" } else { "New Playlist" };
                    self.dialog = Some(Dialog::Create { parent, kind, name: name.into() });
                }
                Action::AskRename { id, name } => self.dialog = Some(Dialog::Rename { id, name }),
                Action::AskSmartList { parent, id } => {
                    let editor = match &id {
                        None => Some(crate::smartlist_ui::SmartEditor::new_list(parent)),
                        Some(id) => self.library.playlists.iter().find(|p| &p.id == id).map(|node| {
                            crate::smartlist_ui::SmartEditor::edit(node, self.library.smart_lists.get(id).map(String::as_str))
                        }),
                    };
                    self.dialog = editor.map(|e| Dialog::Smart(Box::new(e)));
                }
                Action::AskRemoveFromCollection(ids) => self.dialog = Some(Dialog::ConfirmRemove { ids }),
                Action::Export { playlist, device } => self.start_export(playlist, device),
                Action::AskRemoveFromDevice { device, playlists, tracks } => self.ask_remove_from_device(device, playlists, tracks),
                Action::ImportFromDevice { device, playlist, tracks } => self.import_from_device(device, playlist, tracks),
                Action::ImportHistories { device } => self.import_histories(device),
                Action::AskMySettings { device } => {
                    if let Some(d) = self.devices.list.get(device) {
                        let dialog = crate::mysettings_ui::MySettingsDialog::open(device, &d.name, &d.root, d.write_refusal.clone());
                        self.dialog = Some(Dialog::MySettings(Box::new(dialog)));
                    }
                }
                Action::ImportFromXml { list, tracks } => self.import_from_xml(list, tracks),
                Action::AnalyzeTracks(ids) => self.analyze(&ids, true),
                Action::CancelAnalysis => {
                    if let Some(q) = &self.analysis {
                        q.cancel();
                    }
                    self.edit_status = Some("Analysis cancelled".into());
                }
                Action::CancelExport => {
                    if let Some(job) = &self.export {
                        job.cancel();
                    }
                }
            }
        }
    }

    fn start_export(&mut self, playlist: DbId, device: usize) {
        if self.export.is_some() {
            self.edit_status = Some("An export is already running".into());
            return;
        }
        let (Some(source), Some(target)) = (self.export_source.clone(), self.devices.list.get(device)) else {
            return;
        };
        if let Some(reason) = &target.write_refusal {
            self.edit_status = Some(reason.clone());
            return;
        }
        self.export = Some(ExportJob::start(&self.ctx, source, playlist, target.root.clone(), target.name.clone()));
    }

    fn ask_remove_from_device(&mut self, device: usize, playlists: Vec<Vec<String>>, tracks: Vec<PathBuf>) {
        let Some(target) = self.devices.list.get(device) else { return };
        if let Some(reason) = &target.write_refusal {
            self.edit_status = Some(reason.clone());
            return;
        }
        if self.export.is_some() || self.remove.is_some() {
            self.edit_status = Some("Wait for the running device job to finish".into());
            return;
        }
        let tracks: Vec<String> = tracks.iter().filter_map(|p| device_remove::device_path(&target.root, p)).collect();
        if playlists.is_empty() && tracks.is_empty() {
            return;
        }
        self.dialog = Some(Dialog::ConfirmDeviceRemove { device, playlists, tracks, orphans: true });
    }

    fn import_from_device(&mut self, device: usize, playlist: Option<Vec<String>>, tracks: Vec<PathBuf>) {
        let Some(target) = self.devices.list.get(device) else { return };
        if !self.editing() {
            self.edit_status = Some("Turn on editing first".into());
            return;
        }
        let Some(copy_to) = self.args.import_folder.clone().or_else(mb_library::device_import::default_copy_folder) else {
            return;
        };
        match crate::device_import::import_command(&target.root, playlist.as_deref(), &tracks, copy_to) {
            Ok(command) => {
                self.device_import = Some((target.root.clone(), target.write_refusal.is_none()));
                self.edit_status = Some(format!("Importing from {}…", target.name));
                self.run(command);
            }
            Err(e) => self.edit_status = Some(format!("Import failed: {e}")),
        }
    }

    fn import_histories(&mut self, device: usize) {
        let Some(target) = self.devices.list.get(device) else { return };
        if !self.editing() {
            self.edit_status = Some("Turn on editing first".into());
            return;
        }
        match crate::device_import::histories_command(&target.root) {
            Ok(command) => {
                self.device_import = Some((target.root.clone(), target.write_refusal.is_none()));
                self.run(command);
            }
            Err(e) => self.edit_status = Some(format!("History import: {e}")),
        }
    }

    /// Parses a rekordbox.xml in the background; it then shows as "rekordbox xml".
    fn open_xml(&mut self, path: PathBuf) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::xml_bridge::open(&path));
            ctx.request_repaint();
        });
        self.xml_loading = Some(rx);
    }

    fn poll_xml(&mut self) {
        if let Some(rx) = &self.xml_loading
            && let Ok(result) = rx.try_recv()
        {
            self.xml_loading = None;
            match result {
                Ok(xml) => {
                    self.edit_status = Some(format!("rekordbox xml: {} tracks", xml.snapshot.tracks.len()));
                    self.xml = Some(xml);
                    if self.source.library == LibraryRef::Xml {
                        self.browser.invalidate();
                    }
                }
                Err(e) => self.edit_status = Some(format!("rekordbox xml: {e}")),
            }
        }
        if let Some(job) = &mut self.xml_export
            && let Some(result) = job.poll()
        {
            self.edit_status = Some(result.unwrap_or_else(|e| format!("XML export failed: {e}")));
            self.xml_export = None;
        }
    }

    /// "Import to Collection" on the rekordbox xml: a list (with its tracks) or selected rows.
    /// Asks rekordbox's questions first when they apply.
    fn import_from_xml(&mut self, list: Option<DbId>, tracks: Vec<usize>) {
        if !self.editing() {
            self.edit_status = Some("Turn on editing first".into());
            return;
        }
        let Some(xml) = &self.xml else { return };
        let import = match &list {
            Some(id) => match xml.import_for_list(id) {
                Some(i) => i,
                None => return,
            },
            None => xml.import_for_tracks(&tracks),
        };
        let existing = crate::xml_bridge::already_in_collection(&import, &self.library);
        let same_name = crate::xml_bridge::same_name_lists(&import, &self.library);
        if existing > 0 || !same_name.is_empty() {
            self.dialog = Some(Dialog::ConfirmXmlImport { import: Box::new(import), existing, same_name });
        } else {
            self.run(Command::ImportXml { import });
        }
    }

    fn start_remove(&mut self, device: usize, playlists: Vec<Vec<String>>, tracks: Vec<String>, orphans: bool) {
        let Some(target) = self.devices.list.get(device) else { return };
        // On-stick files the local library still points at stay (`delete_device_song_file`).
        let local_paths = self.library.tracks.iter().filter(|t| t.path.starts_with(&target.root)).map(|t| t.path.clone()).collect();
        let request = mb_usb::RemoveRequest { playlists, tracks, delete_orphan_tracks: orphans, local_paths };
        self.remove = Some(RemoveJob::start(&self.ctx, target.root.clone(), target.name.clone(), request));
    }

    fn poll_remove(&mut self) {
        let Some(job) = &mut self.remove else { return };
        if let Some(result) = job.poll() {
            let root = job.device_root.clone();
            self.edit_status = Some(match result {
                Ok(summary) => format!("{summary} on {}", job.device_name),
                Err(e) => format!("Remove failed: {e}"),
            });
            self.remove = None;
            self.devices.reload(&self.ctx, &root);
        }
    }

    fn poll_export(&mut self) {
        let Some(job) = &mut self.export else { return };
        if let Some(result) = job.poll() {
            let root = job.device_root.clone();
            self.edit_status = Some(match result {
                Ok(summary) => format!("{summary} to {}", job.device_name),
                Err(e) => format!("Export failed: {e}"),
            });
            self.export = None;
            self.devices.reload(&self.ctx, &root);
        }
    }

    /// Queues collection tracks for musicbox's analysis (`analyse-track.md`). Locked tracks
    /// (`Analysed` bit 0x80) and missing files are skipped. `explicit`: the user asked (messages).
    fn analyze(&mut self, ids: &[DbId], explicit: bool) {
        if !self.editing() {
            if explicit {
                self.edit_status = Some("Turn on editing first".into());
            }
            return;
        }
        let jobs: Vec<crate::analysis_jobs::Job> = ids
            .iter()
            .filter_map(|id| self.library.tracks.iter().find(|t| t.id == *id))
            .filter(|t| t.analysed & 0x80 == 0 && t.path.is_file())
            .map(|t| crate::analysis_jobs::Job {
                content: t.id.clone(),
                audio: t.path.clone(),
                file_name: if t.file_name.is_empty() {
                    t.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                } else {
                    t.file_name.clone()
                },
                title: t.title.clone(),
                stored_dat: t.analysis_path.clone(),
                stored_bpm: t.bpm_x100.map(i64::from),
            })
            .collect();
        if jobs.is_empty() {
            if explicit {
                self.edit_status = Some("Nothing to analyse (locked or missing files)".into());
            }
            return;
        }
        let settings = self.analysis_settings.clone();
        let ctx = self.ctx.clone();
        let queue = self.analysis.get_or_insert_with(|| {
            crate::analysis_jobs::AnalysisQueue::new(&ctx, settings, crate::analysis_jobs::AnalysisQueue::default_threads())
        });
        queue.enqueue(jobs);
    }

    /// With `AutoAnalysis` on: tracks without any analysis (just imported) are queued once.
    fn queue_unanalysed(&mut self) {
        if !self.auto_analysis || !self.editing() {
            return;
        }
        let ids: Vec<DbId> = self
            .library
            .tracks
            .iter()
            .filter(|t| t.analysed == 0 && t.analysis_path.is_none() && !self.auto_queued.contains(&t.id))
            .map(|t| t.id.clone())
            .collect();
        self.auto_queued.extend(ids.iter().cloned());
        self.analyze(&ids, false);
    }

    /// Finished analyses go to the editor; the selected track jumps the queue.
    fn poll_analysis(&mut self) {
        let Some(queue) = &self.analysis else { return };
        if !self.editing() {
            queue.cancel();
            return;
        }
        if let Some((LibraryRef::Local, i)) = self.browser.selected
            && let Some(t) = self.library.tracks.get(i)
        {
            queue.prioritize(&t.id);
        }
        let finished = queue.poll();
        for f in finished {
            match f.result {
                Ok(analysis) => {
                    // The vocal pass and the embedding need rekordbox's install (models,
                    // statistics); without it the analysis completes and keeps an earlier PVDI
                    // and `.3EX`.
                    if let Some(e) = &analysis.vocal_error {
                        self.analysis_note = Some(format!("no vocal analysis for {}: {e}", f.title));
                    } else if let Some(e) = &analysis.embedding_error {
                        self.analysis_note = Some(format!("no track embedding for {}: {e}", f.title));
                    }
                    self.run(Command::ApplyAnalysis { content: f.content, analysis })
                }
                Err(e) => self.edit_status = Some(format!("Analysis of {} failed: {e}", f.title)),
            }
        }
    }

    fn run(&mut self, command: Command) {
        self.request(Request::Run(command));
    }

    fn request(&mut self, request: Request) {
        match &mut self.editor {
            Some(editor) if editor.ready => editor.send(request),
            _ => self.edit_status = Some("Turn on editing first".into()),
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.dialog else { return };
        let mut submit = false;
        let mut cancel = false;
        let title = match dialog {
            Dialog::Create { kind: NodeKind::Folder, .. } => "New Folder",
            Dialog::Create { .. } => "New Playlist",
            Dialog::Rename { .. } => "Rename",
            Dialog::ConfirmRemove { .. } => "Remove from Collection",
            Dialog::ConfirmDeviceRemove { .. } => "Remove from Device",
            Dialog::Smart(editor) => editor.title(),
            Dialog::XmlExport { .. } => "Export Collection in xml format",
            Dialog::XmlOpen { .. } => "Open rekordbox xml",
            Dialog::ConfirmXmlImport { .. } => "Import from rekordbox xml",
            Dialog::MySettings(_) => "My Settings",
        };
        egui::Window::new(title).collapsible(false).resizable(false).min_width(380.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            match dialog {
                Dialog::Create { name, .. } | Dialog::Rename { name, .. } => {
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let edit = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY).margin(egui::vec2(8.0, 5.0)));
                    if !edit.has_focus() && !enter {
                        edit.request_focus();
                    }
                    submit |= enter && !name.trim().is_empty();
                }
                Dialog::ConfirmDeviceRemove { device, playlists, tracks, orphans } => {
                    let name = self.devices.list.get(*device).map(|d| d.name.as_str()).unwrap_or("the device");
                    let what = match (playlists.len(), tracks.len()) {
                        (0, n) => format!("{n} track(s)"),
                        (1, 0) => format!("the playlist \"{}\"", playlists[0].join(" / ")),
                        (p, 0) => format!("{p} playlists"),
                        (p, n) => format!("{p} playlist(s) and {n} track(s)"),
                    };
                    ui.label(format!("Remove {what} from {name}?"));
                    ui.weak("Files that no other list or library uses are deleted from the device.");
                    if !playlists.is_empty() {
                        ui.checkbox(orphans, "Also delete tracks no other playlist uses");
                    }
                }
                Dialog::ConfirmRemove { ids } => {
                    ui.label(format!("Remove {} track(s) from the collection? Audio files stay on disk.", ids.len()));
                }
                Dialog::Smart(editor) => editor.show(ui, &self.library.my_tags),
                Dialog::MySettings(dialog) => {
                    ui.label(format!("Settings stored on {}", dialog.name));
                    dialog.show(ui);
                }
                Dialog::XmlExport { path } | Dialog::XmlOpen { path } => {
                    ui.label("File:");
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let edit = ui.add(egui::TextEdit::singleline(path).desired_width(460.0).margin(egui::vec2(8.0, 5.0)));
                    if !edit.has_focus() && !enter {
                        edit.request_focus();
                    }
                    submit |= enter && !path.trim().is_empty();
                }
                Dialog::ConfirmXmlImport { import, existing, same_name } => {
                    ui.label(format!("Import {} track(s) into the collection?", import.tracks.len()));
                    if *existing > 0 {
                        ui.label(format!("{existing} of them already exist in the current collection."));
                        ui.checkbox(&mut import.update_existing, "Load the information of the library being imported into them");
                        ui.weak("Beat grids are not loaded into tracks locked against analysis.");
                    }
                    if !same_name.is_empty() {
                        ui.label(format!("Lists with the same name already exist: {}", same_name.join(", ")));
                        ui.checkbox(&mut import.replace_same_name, "Replace them with the one being imported");
                    }
                }
            }
            ui.add_space(16.0);
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 28.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                submit |= ui.add(theme::primary_button("OK")).clicked();
                cancel |= ui.add(theme::secondary_button("Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if submit && matches!(&self.dialog, Some(Dialog::Smart(e)) if !e.valid()) {
            self.edit_status = Some("Every condition needs a value".into());
            submit = false;
        }
        if submit {
            let command = match self.dialog.take() {
                Some(Dialog::Create { parent, kind, name }) => Command::CreatePlaylist { parent, name, kind },
                Some(Dialog::Rename { id, name }) => Command::RenamePlaylist { id, name },
                Some(Dialog::ConfirmRemove { ids }) => Command::RemoveTracks { ids },
                Some(Dialog::Smart(editor)) => {
                    for command in editor.commands() {
                        self.run(command);
                    }
                    return;
                }
                Some(Dialog::ConfirmDeviceRemove { device, playlists, tracks, orphans }) => {
                    self.start_remove(device, playlists, tracks, orphans);
                    return;
                }
                Some(Dialog::XmlExport { path }) => {
                    if let Some(source) = &self.export_source {
                        let path = PathBuf::from(path.trim());
                        self.xml_export = Some(crate::xml_bridge::XmlExportJob::start(&self.ctx, source, self.library.clone(), path));
                    }
                    return;
                }
                Some(Dialog::XmlOpen { path }) => {
                    self.open_xml(PathBuf::from(path.trim()));
                    return;
                }
                Some(Dialog::ConfirmXmlImport { import, .. }) => Command::ImportXml { import: *import },
                Some(Dialog::MySettings(dialog)) => {
                    if dialog.read_only.is_some() || dialog.settings.changes().is_empty() {
                        return;
                    }
                    match dialog.save() {
                        Ok(n) => self.edit_status = Some(format!("My Settings: wrote {n} file(s) to {}", dialog.name)),
                        Err(e) => {
                            self.edit_status = Some(format!("My Settings not saved: {e}"));
                            self.dialog = Some(Dialog::MySettings(dialog));
                        }
                    }
                    return;
                }
                None => return,
            };
            self.run(command);
        } else if cancel {
            self.dialog = None;
        }
    }

    /// ⌘Z / ⇧⌘Z, and audio files dropped from Finder (import into the collection).
    fn global_input(&mut self, ctx: &egui::Context) -> Vec<Action> {
        let mut actions = Vec::new();
        let dropped = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect::<Vec<_>>()
        });
        let (mut undo, mut redo) = (false, false);
        if self.dialog.is_none() {
            use egui::{Key, KeyboardShortcut, Modifiers};
            ctx.input_mut(|i| {
                // Redo first: the plain ⌘Z shortcut also matches when shift is held.
                redo = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z))
                    || i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Y));
                undo = !redo && i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z));
            });
        }
        if self.dialog.is_none() {
            if undo {
                actions.push(Action::Undo);
            }
            if redo {
                actions.push(Action::Redo);
            }
        }
        if !dropped.is_empty() {
            actions.push(Action::Run(Command::ImportFiles { paths: dropped }));
        }
        actions
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_loading();
        self.devices.poll(ui.ctx());
        if let LibraryRef::Device(i) = self.source.library
            && i >= self.devices.list.len()
        {
            self.source = Source { library: LibraryRef::Local, list: List::Collection };
        }

        let ctx = ui.ctx().clone();
        self.poll_editor(&ctx);
        self.poll_export();
        self.poll_remove();
        self.poll_xml();
        self.poll_analysis();
        let mut actions = self.global_input(&ctx);
        actions.extend(self.run_menu_commands(&ctx));

        egui::Panel::top("top-bar").exact_size(self.title_bar.height).frame(theme::bar_frame(theme::BG_DECK, 14)).show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").exact_size(theme::STATUS_BAR).frame(theme::bar_frame(theme::BG_DECK, 14)).show(ui, |ui| self.status(ui));
        let local_track = self.browser.selected.is_some_and(|(library, i)| library == LibraryRef::Local && i < self.library.tracks.len());
        let controls = local_track && self.waves_ready();
        let deck_frame = egui::Frame::new().fill(theme::BG_DECK).inner_margin(egui::Margin::symmetric(14, deck::PAD as i8));
        let deck_actions = egui::Panel::top("deck").exact_size(deck::height(controls)).frame(deck_frame).show(ui, |ui| self.deck(ui)).inner;
        actions.extend(deck_actions);
        let editing = self.editing();
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(240.0)
            .size_range(180.0..=420.0)
            .frame(egui::Frame::new().fill(theme::BG_PANEL))
            .show(ui, |ui| {
                let xml = self.xml.as_ref().map(|x| (x.path.as_path(), &x.snapshot));
                actions.extend(tree::show(ui, &self.library, &self.devices, xml, &mut self.source, editing))
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG_PANEL)).show(ui, |ui| {
            let library = match self.source.library {
                LibraryRef::Local => &self.library,
                LibraryRef::Device(i) => self.devices.list.get(i).and_then(|d| d.snapshot()).unwrap_or(&EMPTY),
                LibraryRef::Xml => self.xml.as_ref().map_or(&*EMPTY, |x| &x.snapshot),
            };
            let edit = browser::EditCtx { enabled: editing, playlists: &self.library.playlists };
            actions.extend(browser::show(ui, library, &self.source, &mut self.browser, &edit));
        });
        // The table drops its selection when the snapshot changes; put the deck's track back.
        if let Some(id) = self.reselect.take()
            && self.source.library == LibraryRef::Local
            && let Some(i) = self.library.tracks.iter().position(|t| t.id == id)
        {
            self.browser.selected = Some((LibraryRef::Local, i));
            self.browser.selection.insert(i);
        }
        self.dialogs(&ctx);
        self.apply(actions);
    }
}
