use crate::actions::Action;
use crate::devices::{DeviceState, Devices};
use crate::editor::{Editor, Reply, Request};
use crate::device_remove::{self, RemoveJob};
use crate::export::{self, ExportJob};
use crate::library::{LoadResult, spawn_load};
use crate::{Args, browser, cue_panel, tree, waveform};
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

/// Modal dialogs that collect input before a command runs.
enum Dialog {
    Create { parent: Option<DbId>, kind: NodeKind, name: String },
    Rename { id: DbId, name: String },
    ConfirmRemove { ids: Vec<DbId> },
    /// Remove lists (name paths) and tracks (device paths) from a device.
    ConfirmDeviceRemove { device: usize, playlists: Vec<Vec<String>>, tracks: Vec<String>, orphans: bool },
    /// The intelligent playlist editor.
    Smart(Box<crate::smartlist_ui::SmartEditor>),
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
    ctx: egui::Context,
    pub devices: Devices,
    pub source: Source,
    pub browser: browser::State,
    waves: HashMap<PathBuf, Option<waveform::TrackWaves>>,
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
            editor: None,
            dialog: None,
            edit_status: None,
            last_guard_check: None,
            ctx: ui_ctx,
            devices,
            source: Source { library: LibraryRef::Local, list: List::Collection },
            browser: browser::State::default(),
            waves: HashMap::new(),
            player: None,
            wave_style: waveform::Style::Rgb,
            cues: HashMap::new(),
            deck_edit: cue_panel::State::default(),
            reselect: None,
        }
    }

    pub fn snapshot(&self, library: LibraryRef) -> &LibrarySnapshot {
        match library {
            LibraryRef::Local => &self.library,
            LibraryRef::Device(i) => self.devices.list.get(i).and_then(|d| d.snapshot()).unwrap_or(&EMPTY),
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
                    self.export_source = Some(export::Source {
                        db_path: loaded.db_path.clone(),
                        passphrase: loaded.passphrase.clone(),
                        analysis_root: loaded.analysis_root,
                        settings_root: loaded.settings_root,
                    });
                    self.local_db = Some((loaded.db_path, loaded.passphrase));
                    self.browser.invalidate();
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// Selected-track header: title/artist, the overview and detail waveforms, and for collection
    /// tracks the cue and grid controls (`cue_panel`).
    fn deck(&mut self, ui: &mut egui::Ui) -> Vec<Action> {
        let selected = self.browser.selected;
        let track = selected.and_then(|(library, i)| self.snapshot(library).tracks.get(i)).cloned();
        let Some(track) = track else {
            ui.centered_and_justified(|ui| ui.weak("No track selected"));
            return Vec::new();
        };
        let local = selected.is_some_and(|(library, _)| library == LibraryRef::Local);
        let editing = self.editing();
        self.deck_edit.follow(&track.id, track.bpm());
        let waves = track.analysis_path.as_ref().and_then(|path| {
            self.waves.entry(path.clone()).or_insert_with(|| waveform::load(path)).as_ref()
        });
        let wave_style = &mut self.wave_style;
        let player = self.player.as_ref().filter(|(p, _)| *p == track.path).map(|(_, pl)| pl);
        let playing = player.is_some_and(|p| p.is_playing());
        let playhead = player.map(|p| p.position_ms());
        let position = playhead.unwrap_or(self.deck_edit.cursor_ms);
        if playing {
            ui.ctx().request_repaint();
        }
        let db_cues: &[mb_library::Cue] = if local { self.cues.get(&track.id).map_or(&[], Vec::as_slice) } else { &[] };
        let marks = match (local, waves) {
            (true, _) => cue_panel::marks(db_cues),
            (false, Some(w)) => w.cues.clone(),
            (false, None) => Vec::new(),
        };
        let duration = cue_panel::duration_ms(&track, waves);
        let mut play_clicked = false;
        let mut seek_to = None;
        let mut jump = None;
        let mut actions = Vec::new();
        let deck_edit = &mut self.deck_edit;
        ui.horizontal(|ui| {
            artwork(ui, track.artwork_path.as_deref(), 76.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    play_clicked = ui.button(if playing { "Pause" } else { "Play" }).clicked();
                    ui.strong(&track.title);
                    ui.label(&track.artist);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.selectable_value(wave_style, waveform::Style::ThreeBand, "3-Band");
                        ui.selectable_value(wave_style, waveform::Style::Rgb, "RGB");
                    });
                });
                match waves {
                    Some(w) => seek_to = waveform::show_overview(ui, w, &marks, duration, *wave_style, Some(position)),
                    None => {
                        ui.weak("Not analysed");
                    }
                }
            });
        });
        if local && let Some(w) = waves {
            jump = waveform::show_detail(ui, w, &marks, position, 4_000.0);
            actions = cue_panel::controls(ui, &track, db_cues, &w.grid, duration, position, editing, deck_edit, &mut jump);
        }
        if let Some(ms) = jump {
            // Moving the position: the open player seeks (and keeps its play state), otherwise
            // only the cursor moves; nothing starts playing.
            match self.player.as_ref().filter(|(p, _)| *p == track.path) {
                Some((_, player)) if player.is_playing() => player.play_from_ms(ms),
                Some((_, player)) => player.seek_ms(ms),
                None => self.deck_edit.cursor_ms = ms,
            }
        }
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

    fn status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            self.editing_switch(ui);
            ui.separator();
            if self.loading.is_some() {
                ui.spinner();
                ui.label("Loading library…");
            } else if let Some(e) = &self.error {
                ui.colored_label(egui::Color32::from_rgb(0xFF, 0x50, 0x50), e);
            } else {
                ui.label(format!("{} tracks", self.browser.visible_count()));
            }
            if let LibraryRef::Device(i) = self.source.library
                && let Some(DeviceState::Failed(e)) = self.devices.list.get(i).map(|d| &d.state)
            {
                ui.colored_label(egui::Color32::from_rgb(0xFF, 0xB9, 0x37), e);
            }
            if let Some(job) = &self.export {
                ui.separator();
                ui.spinner();
                let text = match &job.progress {
                    Some(p) => format!(
                        "Exporting to {}: {}/{} {}",
                        job.device_name,
                        p.tracks_done,
                        p.tracks_total,
                        p.current.as_deref().unwrap_or("")
                    ),
                    None => format!("Exporting to {}…", job.device_name),
                };
                ui.label(text);
                if ui.small_button("Cancel").clicked() {
                    job.cancel();
                }
            } else if let Some(job) = &self.remove {
                ui.separator();
                ui.spinner();
                ui.label(format!("Removing from {}…", job.device_name));
            } else if let Some(msg) = &self.edit_status {
                ui.separator();
                ui.weak(msg);
            }
        });
    }

    fn editing(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.ready)
    }

    /// Off by default; turning it on opens the write connection (which backs the library up).
    fn editing_switch(&mut self, ui: &mut egui::Ui) {
        let mut on = self.editor.is_some();
        let label = match &self.editor {
            Some(e) if !e.ready => "Editing (starting…)",
            Some(_) => "Editing",
            None => "Read-only",
        };
        if ui.toggle_value(&mut on, label).changed() {
            if on {
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
        if self.editor.is_some() && self.last_guard_check.is_none_or(|t| t.elapsed().as_secs() >= 2) {
            self.last_guard_check = Some(std::time::Instant::now());
            let running = mb_masterdb::running_rekordbox();
            if !running.is_empty() {
                self.editor = None;
                self.edit_status = Some(format!("{} started; editing turned off", running.join(", ")));
                return;
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
        let Some(editor) = &mut self.editor else { return };
        let mut reload = false;
        for reply in editor.poll() {
            match reply {
                Reply::Ready { backup } => self.edit_status = Some(format!("Editing on — backup in {}", backup.display())),
                Reply::Failed(e) => {
                    self.edit_status = Some(e);
                    self.editor = None;
                    return;
                }
                Reply::Done { message, changed } => {
                    let note = self.device_import_note.take();
                    self.edit_status = Some(match note {
                        Some(n) => format!("{message}; {n}"),
                        None => message,
                    });
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
        };
        egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            match dialog {
                Dialog::Create { name, .. } | Dialog::Rename { name, .. } => {
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let edit = ui.text_edit_singleline(name);
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
            }
            ui.horizontal(|ui| {
                submit |= ui.button("OK").clicked();
                cancel |= ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
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

/// Track artwork, preferring the 240 px `_m` variant that rekordbox writes next to the small one.
fn artwork(ui: &mut egui::Ui, path: Option<&std::path::Path>, size: f32) {
    let larger = path.map(|p| {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        p.with_file_name(format!("{stem}_m.jpg"))
    });
    let chosen = larger.filter(|p| p.is_file()).or_else(|| path.map(|p| p.to_path_buf())).filter(|p| p.is_file());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    match chosen {
        Some(p) => {
            egui::Image::new(format!("file://{}", p.display())).corner_radius(2.0).paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgb(0x30, 0x30, 0x30));
        }
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
        let mut actions = self.global_input(&ctx);

        egui::Panel::bottom("status").exact_size(22.0).show(ui, |ui| self.status(ui));
        let local_track = self.browser.selected.is_some_and(|(library, _)| library == LibraryRef::Local);
        let deck_actions = egui::Panel::top("deck").exact_size(if local_track { 186.0 } else { 96.0 }).show(ui, |ui| self.deck(ui)).inner;
        actions.extend(deck_actions);
        let editing = self.editing();
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(230.0)
            .size_range(160.0..=420.0)
            .show(ui, |ui| actions.extend(tree::show(ui, &self.library, &self.devices, &mut self.source, editing)));
        egui::CentralPanel::default().show(ui, |ui| {
            let library = match self.source.library {
                LibraryRef::Local => &self.library,
                LibraryRef::Device(i) => self.devices.list.get(i).and_then(|d| d.snapshot()).unwrap_or(&EMPTY),
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
