use crate::actions::Action;
use crate::devices::{DeviceState, Devices};
use crate::editor::{Editor, Reply, Request};
use crate::export::{self, ExportJob};
use crate::library::{LoadResult, spawn_load};
use crate::{Args, browser, tree, waveform};
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
                    self.library = loaded.snapshot;
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

    /// Selected-track header: title/artist and the overview waveform.
    fn deck(&mut self, ui: &mut egui::Ui) {
        let track = self
            .browser
            .selected
            .and_then(|(library, i)| self.snapshot(library).tracks.get(i))
            .cloned();
        let Some(track) = track else {
            ui.centered_and_justified(|ui| ui.weak("No track selected"));
            return;
        };
        let waves = track.analysis_path.as_ref().and_then(|path| {
            self.waves.entry(path.clone()).or_insert_with(|| waveform::load(path)).as_ref()
        });
        let wave_style = &mut self.wave_style;
        let player = self.player.as_ref().filter(|(p, _)| *p == track.path).map(|(_, pl)| pl);
        let playing = player.is_some_and(|p| p.is_playing());
        let playhead = player.map(|p| p.position_ms());
        if playing {
            ui.ctx().request_repaint();
        }
        let mut play_clicked = false;
        let mut seek_to = None;
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
                    Some(w) => seek_to = waveform::show_overview(ui, w, track.length_seconds.unwrap_or(0) * 1000, *wave_style, playhead),
                    None => {
                        ui.weak("Not analysed");
                    }
                }
            });
        });
        let space = ui.ctx().input(|i| i.key_pressed(egui::Key::Space)) && !ui.ctx().egui_wants_keyboard_input();
        if play_clicked || space || seek_to.is_some() {
            self.preview(&track.path, play_clicked || space, seek_to);
        }
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
                    self.editor = Some(Editor::start(ui.ctx(), db, passphrase, self.args.backup_root.clone()));
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
                    self.edit_status = Some(message);
                    reload |= changed;
                }
            }
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
                Action::AskRemoveFromCollection(ids) => self.dialog = Some(Dialog::ConfirmRemove { ids }),
                Action::Export { playlist, device } => self.start_export(playlist, device),
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
                Dialog::ConfirmRemove { ids } => {
                    ui.label(format!("Remove {} track(s) from the collection? Audio files stay on disk.", ids.len()));
                }
            }
            ui.horizontal(|ui| {
                submit |= ui.button("OK").clicked();
                cancel |= ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if submit {
            let command = match self.dialog.take() {
                Some(Dialog::Create { parent, kind, name }) => Command::CreatePlaylist { parent, name, kind },
                Some(Dialog::Rename { id, name }) => Command::RenamePlaylist { id, name },
                Some(Dialog::ConfirmRemove { ids }) => Command::RemoveTracks { ids },
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
        let mut actions = self.global_input(&ctx);

        egui::Panel::bottom("status").exact_size(22.0).show(ui, |ui| self.status(ui));
        egui::Panel::top("deck").exact_size(96.0).show(ui, |ui| self.deck(ui));
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
        self.dialogs(&ctx);
        self.apply(actions);
    }
}
