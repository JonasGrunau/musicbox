//! Editing the player settings on a device: `MYSETTING.DAT`, `MYSETTING2.DAT`,
//! `DJMMYSETTING.DAT` and `DEVSETTING.DAT`. Spec: `docs/re/mysetting.md` (field map), file
//! layout and CRC in `docs/re/export.md` § 5 ([`crate::settings`]).
//!
//! Every field is described by a [`Field`]: the file, the byte in the data block, rekordbox's
//! label and its options as `(stored value, label)` pairs in rekordbox's menu order. The bytes
//! are kept as they are except for the fields that are changed, so player-written values and
//! the bytes nobody names survive an edit.

use crate::settings::{SettingsFile, SettingsKind};
use crate::{Error, Result};
use std::path::{Path, PathBuf};

/// Where rekordbox shows a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// "My Settings" for CDJ/XDJ players (MYSETTING, MYSETTING2).
    Player,
    /// "My Settings" for DJM mixers (DJMMYSETTING).
    Mixer,
    /// DJ System preferences shown on players (DEVSETTING).
    Device,
}

/// How a field's byte is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// MYSETTING, MYSETTING2, DJMMYSETTING: `0x80 | value`; without bit 7 the field is unset
    /// and the player (and rekordbox's getter) uses `unset`.
    Flagged,
    /// DEVSETTING: the plain value.
    Plain,
}

/// Fields that span two bytes (DJMMYSETTING), decoded as `MySettingsEditor` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Composite {
    /// Settings 3 / 13: 3 holds STEREO / MONO SPLIT, 13 holds NONE; one of them is disabled.
    MonoSplitA,
    /// Settings 7 / 15: 7 holds −24 … −6 dB, 15 (fixed 4) stands for −∞; one is disabled.
    TalkOverLevel,
    /// Settings 16 / 17: the mic echo beat as numerator / denominator.
    MicEchoBeat,
}

/// One setting.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub file: SettingsKind,
    /// Byte in the data block (for a composite: the first of its bytes).
    pub byte: usize,
    pub encoding: Encoding,
    /// `(stored value, label)` in rekordbox's menu order (for a composite: `(index, label)`).
    pub options: &'static [(u8, &'static str)],
    /// The value rekordbox's getter returns when the byte is unset or out of range.
    pub unset: u8,
    pub composite: Option<Composite>,
}

const OFF_ON: &[(u8, &str)] = &[(0, "OFF"), (1, "ON")];
const UNLOCK_LOCK: &[(u8, &str)] = &[(0, "UNLOCK"), (1, "LOCK")];
const ONE_TO_FIVE: &[(u8, &str)] = &[(1, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5")];
const CURVES: &[(u8, &str)] = &[(0, "0"), (1, "1"), (2, "2")];
const MONO_SPLIT: &[(u8, &str)] = &[(0, "STEREO"), (1, "MONO SPLIT"), (2, "NONE")];

const fn player(
    key: &'static str,
    label: &'static str,
    file: SettingsKind,
    byte: usize,
    options: &'static [(u8, &'static str)],
    unset: u8,
) -> Field {
    Field {
        key,
        label,
        group: Group::Player,
        file,
        byte,
        encoding: Encoding::Flagged,
        options,
        unset,
        composite: None,
    }
}

const fn mixer(
    key: &'static str,
    label: &'static str,
    setting: usize,
    options: &'static [(u8, &'static str)],
    unset: u8,
) -> Field {
    Field {
        key,
        label,
        group: Group::Mixer,
        file: SettingsKind::DjmMySetting,
        byte: 12 + setting,
        encoding: Encoding::Flagged,
        options,
        unset,
        composite: None,
    }
}

const fn device(
    key: &'static str,
    label: &'static str,
    byte: usize,
    options: &'static [(u8, &'static str)],
) -> Field {
    Field {
        key,
        label,
        group: Group::Device,
        file: SettingsKind::DevSetting,
        byte,
        encoding: Encoding::Plain,
        options,
        unset: 1,
        composite: None,
    }
}

const MS: SettingsKind = SettingsKind::MySetting;
const MS2: SettingsKind = SettingsKind::MySetting2;

/// Every field musicbox edits, in rekordbox's order (`MySettingsEditor::getItemLabelText`
/// @0x101af29f0 items 0x00–0x2d, then the DJ System DEVSETTING items).
pub const FIELDS: &[Field] = &[
    // CDJ/XDJ (MySettingData getters/setters @0x10024e72c–0x10024f01c).
    player(
        "play_mode",
        "PLAY MODE / AUTO PLAY MODE",
        MS,
        23,
        &[(0, "CONTINUE / ON"), (1, "SINGLE / OFF")],
        1,
    ),
    player("eject_load_lock", "EJECT/LOAD LOCK", MS, 21, UNLOCK_LOCK, 0),
    player("needle_lock", "NEEDLE LOCK", MS, 29, UNLOCK_LOCK, 1),
    player(
        "quantize_beat_value",
        "QUANTIZE BEAT VALUE",
        MS,
        24,
        &[
            (3, "1/8 Beat"),
            (2, "1/4 Beat"),
            (1, "1/2 Beat"),
            (0, "1 Beat"),
        ],
        0,
    ),
    player(
        "hot_cue_auto_load",
        "HOT CUE AUTO LOAD",
        MS,
        25,
        &[(0, "OFF"), (2, "rekordbox SETTING"), (1, "ON")],
        1,
    ),
    player("hot_cue_color", "HOT CUE COLOR", MS, 26, OFF_ON, 0),
    player("gate_cue", "GATE CUE", MS2, 5, OFF_ON, 0),
    player(
        "overwrite_cue_with_hot_cue",
        "OVERWRITE CURRENT CUE WITH HOT CUE (SMART CUE)",
        MS2,
        6,
        OFF_ON,
        0,
    ),
    player(
        "auto_cue_level",
        "AUTO CUE LEVEL",
        MS,
        11,
        &[
            (7, "-78dB"),
            (6, "-72dB"),
            (5, "-66dB"),
            (4, "-60dB"),
            (3, "-54dB"),
            (2, "-48dB"),
            (1, "-42dB"),
            (0, "-36dB"),
            (8, "MEMORY CUE"),
            (9, "HOT CUE"),
        ],
        8,
    ),
    player(
        "time_mode",
        "TIME MODE",
        MS,
        32,
        &[(0, "Elapsed"), (1, "REMAIN")],
        1,
    ),
    player("auto_cue", "AUTO CUE", MS, 34, OFF_ON, 1),
    player(
        "jog_mode",
        "JOG MODE",
        MS,
        33,
        &[(1, "VINYL"), (0, "CDJ")],
        1,
    ),
    player(
        "tempo_range",
        "TEMPO RANGE",
        MS,
        36,
        &[(0, "+/-6"), (1, "+/-10"), (2, "+/-16"), (3, "WIDE")],
        1,
    ),
    player("master_tempo", "MASTER TEMPO", MS, 35, OFF_ON, 0),
    player("quantize", "QUANTIZE", MS, 10, OFF_ON, 1),
    player("sync", "SYNC (BPM/BEAT)", MS, 22, OFF_ON, 0),
    player(
        "phase_meter",
        "PHASE METER",
        MS,
        37,
        &[(0, "TYPE 1"), (1, "TYPE 2")],
        0,
    ),
    player(
        "waveform_phase_meter",
        "WAVEFORM / PHASE METER",
        MS2,
        10,
        &[(0, "WAVEFORM"), (1, "PHASE METER")],
        0,
    ),
    player(
        "waveform_divisions",
        "WAVEFORM DIVISIONS",
        MS2,
        4,
        &[(0, "TIME SCALE"), (1, "PHRASE")],
        1,
    ),
    player(
        "vinyl_speed_adjust",
        "VINYL SPEED ADJUST",
        MS2,
        0,
        &[(0, "TOUCH & RELEASE"), (1, "TOUCH"), (2, "RELEASE")],
        1,
    ),
    player(
        "beat_jump_beat_value",
        "BEAT JUMP BEAT VALUE",
        MS2,
        12,
        &[
            (0, "1/2 Beat"),
            (1, "1 Beat"),
            (2, "2 Beat"),
            (3, "4 Beat"),
            (4, "8 Beat"),
            (5, "16 Beat"),
            (6, "32 Beat"),
            (7, "64 Beat"),
        ],
        5,
    ),
    player(
        "language",
        "LANGUAGE",
        MS,
        12,
        &[
            (1, "English"),
            (2, "French"),
            (3, "German"),
            (4, "Italian"),
            (5, "Dutch"),
            (6, "Spanish"),
            (7, "Russian"),
            (8, "Korean"),
            (9, "Chinese simplified"),
            (10, "Chinese traditional"),
            (11, "Japanese"),
            (12, "Portugal"),
            (13, "Swedish"),
            (14, "Czech"),
            (15, "Hungarian"),
            (16, "Danish"),
            (17, "Greek"),
            (18, "Turkish"),
        ],
        1,
    ),
    player(
        "lcd_brightness",
        "LCD BRIGHTNESS (Main Display)",
        MS,
        9,
        ONE_TO_FIVE,
        3,
    ),
    player(
        "jog_lcd_brightness",
        "JOG LCD BRIGHTNESS (Deck Display)",
        MS2,
        3,
        ONE_TO_FIVE,
        3,
    ),
    player(
        "jog_display_mode",
        "JOG DISPLAY MODE",
        MS2,
        1,
        &[(0, "AUTO"), (1, "INFO"), (2, "SIMPLE"), (3, "ARTWORK")],
        0,
    ),
    player("slip_flashing", "SLIP FLASHING", MS, 16, OFF_ON, 1),
    player("on_air_display", "ON AIR DISPLAY", MS, 8, OFF_ON, 1),
    player(
        "jog_ring_brightness",
        "JOG RING BRIGHTNESS",
        MS,
        14,
        &[(0, "OFF"), (1, "1 (Dark)"), (2, "2 (Bright)")],
        2,
    ),
    player(
        "jog_ring_indicator",
        "JOG RING INDICATOR",
        MS,
        15,
        OFF_ON,
        1,
    ),
    player(
        "disc_slot_illumination",
        "DISC SLOT ILLUMINATION",
        MS,
        20,
        &[(0, "OFF"), (1, "1 (Dark)"), (2, "2 (Bright)")],
        2,
    ),
    player(
        "pad_button_brightness",
        "PAD/BUTTON BRIGHTNESS",
        MS2,
        2,
        &[(1, "1"), (2, "2"), (3, "3"), (4, "4")],
        3,
    ),
    // DJM (generalSettingDefine @0x1007d3ce4; MySettingsEditor::onDJMMySettingChanged @0x101af5d64).
    mixer("ch_fader_curve", "CH FADER CURVE", 0, CURVES, 1),
    mixer(
        "ch_fader_curve_long",
        "CH FADER CURVE (LONG FADER)",
        12,
        CURVES,
        0,
    ),
    mixer("crossfader_curve", "CROSSFADER CURVE", 1, CURVES, 2),
    mixer(
        "headphones_pre_eq",
        "HEADPHONES PRE EQ (A)",
        2,
        &[(0, "POST EQ"), (1, "PRE EQ")],
        0,
    ),
    Field {
        key: "headphones_mono_split_a",
        label: "HEADPHONES MONO SPLIT (A)",
        group: Group::Mixer,
        file: SettingsKind::DjmMySetting,
        byte: 12 + 3,
        encoding: Encoding::Flagged,
        options: MONO_SPLIT,
        unset: 0,
        composite: Some(Composite::MonoSplitA),
    },
    mixer(
        "headphones_mono_split_b",
        "HEADPHONES MONO SPLIT (B)",
        14,
        MONO_SPLIT,
        2,
    ),
    mixer("beat_fx_quantize", "BEAT FX QUANTIZE", 4, OFF_ON, 1),
    mixer(
        "mic_low_cut",
        "MIC LOW CUT",
        5,
        &[(0, "OFF"), (1, "ON(for MC)")],
        1,
    ),
    Field {
        key: "mic_echo_beat",
        label: "MIC ECHO BEAT",
        group: Group::Mixer,
        file: SettingsKind::DjmMySetting,
        byte: 12 + 16,
        encoding: Encoding::Flagged,
        options: &[
            (0, "1/8"),
            (1, "1/4"),
            (2, "1/2"),
            (3, "3/4"),
            (4, "1"),
            (5, "2"),
            (6, "4"),
            (7, "8"),
        ],
        unset: 4,
        composite: Some(Composite::MicEchoBeat),
    },
    mixer(
        "talk_over_mode",
        "TALK OVER MODE",
        6,
        &[(0, "ADVANCED"), (1, "NORMAL")],
        0,
    ),
    Field {
        key: "talk_over_level",
        label: "TALK OVER LEVEL",
        group: Group::Mixer,
        file: SettingsKind::DjmMySetting,
        byte: 12 + 7,
        encoding: Encoding::Flagged,
        options: &[
            (0, "-∞"),
            (1, "-24dB"),
            (2, "-18dB"),
            (3, "-12dB"),
            (4, "-6dB"),
        ],
        unset: 2,
        composite: Some(Composite::TalkOverLevel),
    },
    mixer(
        "midi_ch",
        "MIDI CH",
        8,
        &[
            (0, "1"),
            (1, "2"),
            (2, "3"),
            (3, "4"),
            (4, "5"),
            (5, "6"),
            (6, "7"),
            (7, "8"),
            (8, "9"),
            (9, "10"),
            (10, "11"),
            (11, "12"),
            (12, "13"),
            (13, "14"),
            (14, "15"),
            (15, "16"),
        ],
        0,
    ),
    mixer(
        "midi_button_type",
        "MIDI BUTTON TYPE",
        9,
        &[(0, "TOGGLE"), (1, "TRIGGER")],
        0,
    ),
    mixer(
        "brightness_display",
        "BRIGHTNESS DISPLAY",
        10,
        &[
            (0, "WHITE"),
            (1, "1"),
            (2, "2"),
            (3, "3"),
            (4, "4"),
            (5, "5"),
        ],
        5,
    ),
    mixer(
        "brightness_indicator",
        "BRIGHTNESS INDICATOR",
        11,
        &[(0, "1"), (1, "2"), (2, "3")],
        2,
    ),
    // DJ System (cdjDeviceSetting::DeviceSettingData @0x100c06638–0x100c0688c, DJSystem* pages).
    device(
        "overview_waveform",
        "Type of the Overview Waveform",
        9,
        &[(1, "Half Waveform"), (2, "Full Waveform")],
    ),
    device(
        "waveform_color",
        "Waveform color",
        10,
        &[(1, "BLUE"), (3, "RGB"), (4, "3Band")],
    ),
    device(
        "key_display_format",
        "Key display format",
        12,
        &[(1, "Classic"), (2, "Alphanumeric")],
    ),
    device(
        "waveform_position",
        "Waveform Current Position",
        13,
        &[(1, "CENTER"), (2, "LEFT")],
    ),
    device(
        "device_waveform_divisions",
        "Waveform Divisions",
        14,
        &[(1, "Time Scale"), (2, "Phrase")],
    ),
    device(
        "image_on_jog",
        "Image On-Jog Display",
        15,
        &[(1, "Artwork"), (2, "Custom Image")],
    ),
    device(
        "display_username",
        "Display username on-screen",
        16,
        &[(1, "OFF"), (2, "ON")],
    ),
];

/// Looks a field up by its key.
pub fn field(key: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.key == key)
}

/// Valid range of a DJMMYSETTING setting (`generalSettingDefine` @0x1007d3ce4): (min, max).
fn djm_range(setting: usize) -> (u8, u8) {
    const RANGES: [(u8, u8); 18] = [
        (0, 2),
        (0, 2),
        (0, 1),
        (0, 1),
        (0, 1),
        (0, 1),
        (0, 1),
        (0, 3),
        (0, 15),
        (0, 1),
        (0, 5),
        (0, 2),
        (0, 2),
        (0, 2),
        (0, 2),
        (4, 4),
        (1, 8),
        (1, 8),
    ];
    RANGES.get(setting).copied().unwrap_or((0, 0x7f))
}

/// Mic echo beat per menu index as (numerator, denominator), settings 16 / 17.
const MIC_ECHO: [(u8, u8); 8] = [
    (1, 8),
    (1, 4),
    (1, 2),
    (3, 4),
    (1, 1),
    (2, 1),
    (4, 1),
    (8, 1),
];

/// One of the four files on the device.
#[derive(Debug, Clone)]
pub struct DeviceFile {
    pub path: PathBuf,
    /// The bytes on the device (`None`: missing).
    pub original: Option<Vec<u8>>,
    /// Parsed file; `None` when it is missing (a new one is made on the first change) or does
    /// not parse (then it is left alone and its fields are read-only).
    pub parsed: Option<SettingsFile>,
    /// The file exists but is not one musicbox can rewrite (bad size or CRC, e.g. a
    /// DJMMYSETTING with a Beat FX block).
    pub unreadable: bool,
}

/// The settings of one device.
#[derive(Debug, Clone)]
pub struct MySettings {
    pub files: Vec<(SettingsKind, DeviceFile)>,
}

impl MySettings {
    /// Reads the four files from the device's `PIONEER` (or `.PIONEER`) folder.
    pub fn load(pioneer_dir: &Path) -> Self {
        let files = SettingsKind::ALL
            .iter()
            .map(|&kind| {
                let path = pioneer_dir.join(kind.file_name());
                let original = std::fs::read(&path).ok();
                let parsed = original
                    .as_deref()
                    .and_then(|b| SettingsFile::parse(kind, b));
                let unreadable = original.is_some() && parsed.is_none();
                (
                    kind,
                    DeviceFile {
                        path,
                        original,
                        parsed,
                        unreadable,
                    },
                )
            })
            .collect();
        Self { files }
    }

    fn file(&self, kind: SettingsKind) -> &DeviceFile {
        &self
            .files
            .iter()
            .find(|(k, _)| *k == kind)
            .expect("all four kinds")
            .1
    }

    fn file_mut(&mut self, kind: SettingsKind) -> &mut DeviceFile {
        &mut self
            .files
            .iter_mut()
            .find(|(k, _)| *k == kind)
            .expect("all four kinds")
            .1
    }

    /// The data block in effect: the file's, or rekordbox's defaults for a missing file.
    fn data(&self, kind: SettingsKind) -> Option<Vec<u8>> {
        let f = self.file(kind);
        match (&f.parsed, f.unreadable) {
            (Some(p), _) => Some(p.data.clone()),
            (None, false) => Some(kind.default_data()),
            (None, true) => None,
        }
    }

    /// Whether a field can be changed (its file is missing or readable).
    pub fn editable(&self, field: &Field) -> bool {
        !self.file(field.file).unreadable
    }

    /// The value the player uses: the stored value, or `unset` when the byte is unset (or,
    /// as rekordbox's getters do, out of range). For a composite: the menu index.
    pub fn value(&self, field: &Field) -> Option<u8> {
        let data = self.data(field.file)?;
        let byte = |i: usize| data.get(i).copied().unwrap_or(0);
        let flagged = |i: usize| {
            let b = byte(i);
            (b & 0x80 != 0).then_some(b & 0x7f)
        };
        let djm = |setting: usize| {
            let (min, max) = djm_range(setting);
            flagged(12 + setting).filter(|v| (min..=max).contains(v))
        };
        Some(match (field.composite, field.encoding) {
            (Some(Composite::MonoSplitA), _) => match flagged(12 + 13) {
                Some(v) if v <= 2 => v,
                _ => djm(3).unwrap_or(0),
            },
            (Some(Composite::TalkOverLevel), _) => match (djm(7), flagged(12 + 15)) {
                (Some(v), _) if v < 4 => v + 1,
                (Some(_), _) => field.unset,
                (None, Some(_)) => 0,
                (None, None) => 2,
            },
            (Some(Composite::MicEchoBeat), _) => {
                let pair = (djm(16).unwrap_or(1), djm(17).unwrap_or(1));
                MIC_ECHO
                    .iter()
                    .position(|p| *p == pair)
                    .map_or(4, |i| i as u8)
            }
            (None, Encoding::Plain) => byte(field.byte),
            (None, Encoding::Flagged) if field.file == SettingsKind::DjmMySetting => {
                djm(field.byte - 12).unwrap_or(field.unset)
            }
            (None, Encoding::Flagged) => flagged(field.byte).unwrap_or(field.unset),
        })
    }

    /// The menu index of the current value (`None`: the stored value has no menu entry, e.g.
    /// a value a player wrote that rekordbox does not offer).
    pub fn selected(&self, field: &Field) -> Option<usize> {
        let v = self.value(field)?;
        field.options.iter().position(|(o, _)| *o == v)
    }

    /// Sets the field to the option at `index` of `field.options`.
    pub fn select(&mut self, field: &Field, index: usize) -> Result<()> {
        let (value, _) = *field
            .options
            .get(index)
            .ok_or_else(|| Error::Device(format!("{}: no option {index}", field.key)))?;
        if !self.editable(field) {
            return Err(Error::Device(format!(
                "{} cannot be read; musicbox leaves it alone",
                field.file.file_name()
            )));
        }
        let kind = field.file;
        let mut data = self.data(kind).expect("editable");
        let set = |data: &mut Vec<u8>, i: usize, v: u8| data[i] = 0x80 | v;
        match (field.composite, field.encoding) {
            // MySettingsEditor::onDJMMySettingChanged @0x101af5d64.
            (Some(Composite::MonoSplitA), _) => {
                if value < 2 {
                    set(&mut data, 12 + 3, value);
                    data[12 + 13] = 0;
                } else {
                    set(&mut data, 12 + 13, value);
                    data[12 + 3] = 0;
                }
            }
            (Some(Composite::TalkOverLevel), _) => {
                if value == 0 {
                    data[12 + 7] = 0;
                    set(&mut data, 12 + 15, 4);
                } else {
                    set(&mut data, 12 + 7, value - 1);
                    data[12 + 15] = 0;
                }
            }
            (Some(Composite::MicEchoBeat), _) => {
                let (num, den) = MIC_ECHO[value as usize];
                set(&mut data, 12 + 16, num);
                set(&mut data, 12 + 17, den);
            }
            (None, Encoding::Flagged) => set(&mut data, field.byte, value),
            (None, Encoding::Plain) => data[field.byte] = value,
        }
        let f = self.file_mut(kind);
        match &mut f.parsed {
            Some(p) => p.data = data,
            None => {
                // A missing file is created as rekordbox would write it (header per
                // `SettingsKind`), with the change applied.
                let fresh = crate::settings::device_settings_bytes(kind, None, None)
                    .and_then(|b| SettingsFile::parse(kind, &b))
                    .expect("default file parses");
                f.parsed = Some(SettingsFile { data, ..fresh });
            }
        }
        Ok(())
    }

    /// Files whose bytes differ from the device's, with their new bytes (fresh CRC).
    pub fn changes(&self) -> Vec<(PathBuf, Vec<u8>)> {
        self.files
            .iter()
            .filter_map(|(kind, f)| {
                let bytes = f.parsed.as_ref()?.to_bytes(*kind);
                (f.original.as_deref() != Some(bytes.as_slice())).then(|| (f.path.clone(), bytes))
            })
            .collect()
    }

    /// Writes the changed files to the device (`root` is the volume, for the reference-stick
    /// guard; callers also check the protected-volume list). Each file is checked to be unchanged
    /// since [`MySettings::load`], written next to its name, `F_FULLFSYNC`ed and renamed into
    /// place. Returns the written paths.
    pub fn save(&self, root: &Path) -> Result<Vec<PathBuf>> {
        crate::remove::refuse_reference_stick(root)?;
        let changes = self.changes();
        for (path, _) in &changes {
            let (_, f) = self
                .files
                .iter()
                .find(|(_, f)| &f.path == path)
                .expect("own file");
            match &f.original {
                Some(orig) => crate::remove::check_unchanged(path, orig)?,
                None if path.exists() => {
                    return Err(Error::Device(format!(
                        "{} appeared since it was read",
                        path.display()
                    )));
                }
                None => {}
            }
        }
        let mut written = Vec::new();
        for (path, bytes) in changes {
            crate::exec::write_replace(&path, &bytes)?;
            crate::remove::full_fsync(&path)?;
            written.push(path);
        }
        crate::appledouble::remove_companions(root, written.iter().map(PathBuf::as_path));
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_pioneer(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mb-mysettings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_read_as_rekordbox_getters() {
        let dir = temp_pioneer("defaults");
        let s = MySettings::load(&dir);
        let label = |key: &str| {
            let f = field(key).unwrap();
            f.options[s.selected(f).unwrap()].1
        };
        assert_eq!(label("play_mode"), "SINGLE / OFF");
        assert_eq!(label("auto_cue_level"), "MEMORY CUE");
        assert_eq!(label("tempo_range"), "+/-10");
        assert_eq!(label("quantize_beat_value"), "1 Beat");
        assert_eq!(label("hot_cue_auto_load"), "ON");
        assert_eq!(label("jog_mode"), "VINYL");
        assert_eq!(label("beat_jump_beat_value"), "16 Beat");
        assert_eq!(label("language"), "English");
        assert_eq!(label("lcd_brightness"), "3");
        assert_eq!(label("crossfader_curve"), "2");
        assert_eq!(label("headphones_mono_split_a"), "STEREO");
        assert_eq!(label("headphones_mono_split_b"), "NONE");
        assert_eq!(label("talk_over_level"), "-18dB");
        assert_eq!(label("mic_echo_beat"), "1");
        assert_eq!(label("brightness_display"), "5");
        assert_eq!(label("waveform_color"), "BLUE");
        assert_eq!(label("key_display_format"), "Classic");
        assert!(s.changes().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_option_of_every_field_round_trips() {
        let dir = temp_pioneer("all");
        for f in FIELDS {
            for i in 0..f.options.len() {
                let mut s = MySettings::load(&dir);
                s.select(f, i).unwrap();
                assert_eq!(s.selected(f), Some(i), "{} option {i}", f.key);
                // Through the bytes: parse what would be written.
                for (path, bytes) in s.changes() {
                    std::fs::write(&path, &bytes).unwrap();
                }
                let back = MySettings::load(&dir);
                assert_eq!(
                    back.selected(f),
                    Some(i),
                    "{} option {i} after writing",
                    f.key
                );
                for (k, df) in &back.files {
                    if df.original.is_some() {
                        assert!(df.parsed.is_some(), "{} parses", k.file_name());
                    }
                }
                for k in SettingsKind::ALL {
                    let _ = std::fs::remove_file(dir.join(k.file_name()));
                }
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_change_touches_only_its_bytes() {
        let dir = temp_pioneer("bytes");
        let mut s = MySettings::load(&dir);
        let before = SettingsKind::MySetting.default_data();
        s.select(field("tempo_range").unwrap(), 3).unwrap();
        let changes = s.changes();
        assert_eq!(changes.len(), 1);
        let after = SettingsFile::parse(SettingsKind::MySetting, &changes[0].1)
            .unwrap()
            .data;
        let diff: Vec<usize> = (0..before.len())
            .filter(|&i| before[i] != after[i])
            .collect();
        assert_eq!(diff, [36]);
        assert_eq!(after[36], 0x83);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn composites_disable_their_partner() {
        let dir = temp_pioneer("composite");
        let mut s = MySettings::load(&dir);
        let get = |s: &MySettings| s.data(SettingsKind::DjmMySetting).unwrap();
        s.select(field("headphones_mono_split_a").unwrap(), 2)
            .unwrap();
        assert_eq!((get(&s)[12 + 3], get(&s)[12 + 13]), (0x00, 0x82));
        s.select(field("headphones_mono_split_a").unwrap(), 1)
            .unwrap();
        assert_eq!((get(&s)[12 + 3], get(&s)[12 + 13]), (0x81, 0x00));
        s.select(field("talk_over_level").unwrap(), 0).unwrap();
        assert_eq!((get(&s)[12 + 7], get(&s)[12 + 15]), (0x00, 0x84));
        s.select(field("talk_over_level").unwrap(), 4).unwrap();
        assert_eq!((get(&s)[12 + 7], get(&s)[12 + 15]), (0x83, 0x00));
        s.select(field("mic_echo_beat").unwrap(), 3).unwrap();
        assert_eq!((get(&s)[12 + 16], get(&s)[12 + 17]), (0x83, 0x84));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unreadable_files_are_left_alone() {
        let dir = temp_pioneer("unreadable");
        // A DJMMYSETTING with a Beat FX block (0x480 bytes) is not one musicbox rewrites.
        std::fs::write(dir.join("DJMMYSETTING.DAT"), vec![0u8; 0x480]).unwrap();
        let mut s = MySettings::load(&dir);
        let f = field("crossfader_curve").unwrap();
        assert!(!s.editable(f));
        assert!(s.select(f, 0).is_err());
        assert!(s.changes().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
