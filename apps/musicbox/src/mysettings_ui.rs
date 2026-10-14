//! "My Settings…" on a device: the player, mixer and DJ System settings in the stick's
//! `MYSETTING.DAT`, `MYSETTING2.DAT`, `DJMMYSETTING.DAT` and `DEVSETTING.DAT`
//! (`mb_usb::mysettings`, `docs/re/mysetting.md`). Changes are written to the device only on
//! OK; protected devices are shown read-only.

use mb_usb::mysettings::{FIELDS, Group, MySettings};
use std::path::{Path, PathBuf};

pub struct MySettingsDialog {
    pub device: usize,
    pub name: String,
    pub root: PathBuf,
    pub settings: MySettings,
    /// Why the device must not be written (protected volume), if so.
    pub read_only: Option<String>,
}

/// `<root>/PIONEER`, or `<root>/.PIONEER` when only that one exists (HFS sticks).
fn pioneer_dir(root: &Path) -> PathBuf {
    let hidden = root.join(".PIONEER");
    if hidden.is_dir() && !root.join("PIONEER").is_dir() {
        hidden
    } else {
        root.join("PIONEER")
    }
}

impl MySettingsDialog {
    pub fn open(device: usize, name: &str, root: &Path, read_only: Option<String>) -> Self {
        Self {
            device,
            name: name.to_owned(),
            root: root.to_path_buf(),
            settings: MySettings::load(&pioneer_dir(root)),
            read_only,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        if let Some(reason) = &self.read_only {
            ui.weak(format!("Read-only: {reason}"));
        }
        egui::ScrollArea::vertical()
            .max_height(520.0)
            .show(ui, |ui| {
                for (group, title) in [
                    (Group::Player, "PLAYER"),
                    (Group::Mixer, "MIXER"),
                    (Group::Device, "DJ SYSTEM"),
                ] {
                    ui.add_space(10.0);
                    ui.label(crate::theme::caption(title));
                    ui.add_space(2.0);
                    egui::Grid::new(("mysettings", title))
                        .num_columns(2)
                        .spacing([24.0, 6.0])
                        .show(ui, |ui| {
                            for field in FIELDS.iter().filter(|f| f.group == group) {
                                ui.label(field.label);
                                let editable =
                                    self.read_only.is_none() && self.settings.editable(field);
                                let current = match (
                                    self.settings.selected(field),
                                    self.settings.value(field),
                                ) {
                                    (Some(i), _) => field.options[i].1.to_owned(),
                                    (None, Some(v)) => format!("(value {v})"),
                                    (None, None) => "(file not readable)".to_owned(),
                                };
                                let mut chosen = None;
                                ui.add_enabled_ui(editable, |ui| {
                                    egui::ComboBox::from_id_salt(("mysetting", field.key))
                                        .selected_text(current)
                                        .width(180.0)
                                        .show_ui(ui, |ui| {
                                            let selected = self.settings.selected(field);
                                            for (i, (_, label)) in field.options.iter().enumerate()
                                            {
                                                if ui
                                                    .selectable_label(selected == Some(i), *label)
                                                    .clicked()
                                                {
                                                    chosen = Some(i);
                                                }
                                            }
                                        });
                                });
                                if let Some(i) = chosen {
                                    // Only editable fields reach here; an error leaves the value as it was.
                                    let _ = self.settings.select(field, i);
                                }
                                ui.end_row();
                            }
                        });
                }
            });
        let changed = self.settings.changes().len();
        if changed > 0 {
            ui.weak(format!("OK writes {changed} file(s) to {}.", self.name));
        }
    }

    /// Writes the changed files; returns how many.
    pub fn save(&self) -> Result<usize, String> {
        if let Some(reason) = &self.read_only {
            return Err(reason.clone());
        }
        let running = mb_masterdb::running_rekordbox();
        if !running.is_empty() {
            return Err(format!("Quit {} first", running.join(", ")));
        }
        self.settings
            .save(&self.root)
            .map(|w| w.len())
            .map_err(|e| e.to_string())
    }
}
