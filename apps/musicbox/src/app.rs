use crate::devices::{DeviceState, Devices};
use crate::library::{LoadResult, spawn_load};
use crate::{Args, browser, tree, waveform};
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

pub struct App {
    loading: Option<Receiver<LoadResult>>,
    error: Option<String>,
    pub library: LibrarySnapshot,
    pub devices: Devices,
    pub source: Source,
    pub browser: browser::State,
    waves: HashMap<PathBuf, Option<waveform::TrackWaves>>,
    wave_style: waveform::Style,
}

static EMPTY: std::sync::LazyLock<LibrarySnapshot> = std::sync::LazyLock::new(LibrarySnapshot::default);

impl App {
    pub fn new(ctx: egui::Context, args: Args) -> Self {
        let devices = Devices::new(args.devices.clone());
        Self {
            loading: Some(spawn_load(ctx, args)),
            error: None,
            library: LibrarySnapshot::default(),
            devices,
            source: Source { library: LibraryRef::Local, list: List::Collection },
            browser: browser::State::default(),
            waves: HashMap::new(),
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
                Ok(library) => {
                    self.library = library;
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
        ui.horizontal(|ui| {
            artwork(ui, track.artwork_path.as_deref(), 76.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.strong(&track.title);
                    ui.label(&track.artist);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.selectable_value(wave_style, waveform::Style::ThreeBand, "3-Band");
                        ui.selectable_value(wave_style, waveform::Style::Rgb, "RGB");
                    });
                });
                match waves {
                    Some(w) => waveform::show_overview(ui, w, track.length_seconds.unwrap_or(0) * 1000, *wave_style),
                    None => {
                        ui.weak("Not analysed");
                    }
                }
            });
        });
    }

    fn status(&self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
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
        });
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

        egui::Panel::bottom("status").exact_size(22.0).show(ui, |ui| self.status(ui));
        egui::Panel::top("deck").exact_size(96.0).show(ui, |ui| self.deck(ui));
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(230.0)
            .size_range(160.0..=420.0)
            .show(ui, |ui| tree::show(ui, &self.library, &self.devices, &mut self.source));
        egui::CentralPanel::default().show(ui, |ui| {
            let library = match self.source.library {
                LibraryRef::Local => &self.library,
                LibraryRef::Device(i) => self.devices.list.get(i).and_then(|d| d.snapshot()).unwrap_or(&EMPTY),
            };
            browser::show(ui, library, &self.source, &mut self.browser);
        });
    }
}
