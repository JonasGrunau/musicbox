//! The My Settings field map (`docs/re/mysetting.md`) against real files: the reference stick
//! (player-written MYSETTING2/DEVSETTING from a CDJ-3000, an older DJMMYSETTING) and the oracle
//! stick (rekordbox 7.2.19 defaults). Reading only; edits run on copies in a temp dir. Skipped
//! without `fixtures/`.

use mb_usb::mysettings::{FIELDS, MySettings, field};
use mb_usb::settings::{SettingsFile, SettingsKind};
use std::path::{Path, PathBuf};

fn fixture(rel: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel);
    p.join("MYSETTING.DAT").is_file().then_some(p)
}

fn label(s: &MySettings, key: &str) -> Option<&'static str> {
    let f = field(key).unwrap();
    s.selected(f).map(|i| f.options[i].1)
}

#[test]
fn reference_stick_reads_and_round_trips() {
    let Some(dir) = fixture("stick/PIONEER") else {
        eprintln!("skipped: fixtures/stick is not present");
        return;
    };
    let s = MySettings::load(&dir);
    for (kind, f) in &s.files {
        let parsed = f
            .parsed
            .as_ref()
            .unwrap_or_else(|| panic!("{} parses", kind.file_name()));
        assert_eq!(
            Some(parsed.to_bytes(*kind)),
            f.original,
            "{} round trip",
            kind.file_name()
        );
    }
    assert!(s.changes().is_empty());
    // MYSETTING (written by rekordbox for its own library).
    assert_eq!(label(&s, "lcd_brightness"), Some("5"));
    assert_eq!(label(&s, "auto_cue_level"), Some("-60dB"));
    assert_eq!(label(&s, "eject_load_lock"), Some("LOCK"));
    assert_eq!(label(&s, "hot_cue_color"), Some("ON"));
    assert_eq!(label(&s, "auto_cue"), Some("OFF"));
    assert_eq!(label(&s, "master_tempo"), Some("ON"));
    assert_eq!(label(&s, "tempo_range"), Some("+/-6"));
    // MYSETTING2 written by the CDJ-3000, which leaves several fields unset (bit 7 clear):
    // they read as rekordbox's getter defaults.
    let pad = field("pad_button_brightness").unwrap();
    assert_eq!((s.value(pad), s.selected(pad)), (Some(3), Some(2)));
    assert_eq!(label(&s, "jog_display_mode"), Some("AUTO"));
    assert_eq!(label(&s, "waveform_divisions"), Some("PHRASE"));
    assert_eq!(label(&s, "jog_lcd_brightness"), Some("5"));
    assert_eq!(label(&s, "waveform_phase_meter"), Some("PHASE METER"));
    assert_eq!(label(&s, "beat_jump_beat_value"), Some("16 Beat"));
    // DEVSETTING written by the CDJ-3000.
    assert_eq!(label(&s, "overview_waveform"), Some("Full Waveform"));
    assert_eq!(label(&s, "waveform_color"), Some("RGB"));
    assert_eq!(label(&s, "key_display_format"), Some("Alphanumeric"));
    assert_eq!(label(&s, "waveform_position"), Some("CENTER"));
    assert_eq!(
        s.selected(field("device_waveform_divisions").unwrap()),
        None
    );
    // DJMMYSETTING from an older rekordbox: settings 14-17 unset, so the getter defaults.
    assert_eq!(label(&s, "headphones_mono_split_b"), Some("NONE"));
    assert_eq!(label(&s, "mic_echo_beat"), Some("1"));
    assert_eq!(label(&s, "talk_over_level"), Some("-18dB"));
}

#[test]
fn oracle_stick_reads_as_defaults() {
    let Some(dir) = fixture("oracle-a/rbtest/PIONEER") else {
        eprintln!("skipped: fixtures/oracle-a is not present");
        return;
    };
    let s = MySettings::load(&dir);
    let empty = std::env::temp_dir().join(format!("mb-mysettings-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(&empty).unwrap();
    let defaults = MySettings::load(&empty);
    for f in FIELDS {
        assert_eq!(s.value(f), defaults.value(f), "{}", f.key);
        assert!(s.selected(f).is_some(), "{}", f.key);
    }
    assert!(s.changes().is_empty());
    std::fs::remove_dir_all(&empty).unwrap();
}

#[test]
fn edits_keep_player_headers_and_unknown_bytes() {
    let Some(src) = fixture("stick/PIONEER") else {
        eprintln!("skipped: fixtures/stick is not present");
        return;
    };
    let dir = std::env::temp_dir().join(format!("mb-mysettings-edit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for k in SettingsKind::ALL {
        std::fs::copy(src.join(k.file_name()), dir.join(k.file_name())).unwrap();
    }
    let mut s = MySettings::load(&dir);
    s.select(field("jog_display_mode").unwrap(), 3).unwrap(); // MYSETTING2: ARTWORK
    s.select(field("language").unwrap(), 2).unwrap(); // MYSETTING: German
    s.select(field("waveform_color").unwrap(), 2).unwrap(); // DEVSETTING: 3Band
    s.select(field("talk_over_level").unwrap(), 0).unwrap(); // DJMMYSETTING: -inf
    let written = s.save(&dir).unwrap();
    assert_eq!(written.len(), 4);
    let after = MySettings::load(&dir);
    assert_eq!(label(&after, "jog_display_mode"), Some("ARTWORK"));
    assert_eq!(label(&after, "language"), Some("German"));
    assert_eq!(label(&after, "waveform_color"), Some("3Band"));
    assert_eq!(label(&after, "talk_over_level"), Some("-∞"));
    // Only the edited bytes changed; headers (CDJ-3000 on MYSETTING2/DEVSETTING) are kept.
    let expect_diff: [(SettingsKind, &[usize]); 4] = [
        (SettingsKind::MySetting, &[12]),
        (SettingsKind::MySetting2, &[1]),
        (SettingsKind::DevSetting, &[10]),
        (SettingsKind::DjmMySetting, &[12 + 7, 12 + 15]),
    ];
    for (kind, bytes) in expect_diff {
        let a =
            SettingsFile::parse(kind, &std::fs::read(src.join(kind.file_name())).unwrap()).unwrap();
        let b =
            SettingsFile::parse(kind, &std::fs::read(dir.join(kind.file_name())).unwrap()).unwrap();
        assert_eq!(
            (&a.brand, &a.product, &a.version),
            (&b.brand, &b.product, &b.version)
        );
        let diff: Vec<usize> = (0..a.data.len())
            .filter(|&i| a.data[i] != b.data[i])
            .collect();
        assert_eq!(diff, bytes, "{}", kind.file_name());
    }
    // Unchanged since load is checked: a second save of the old state is refused.
    std::fs::write(
        dir.join("MYSETTING.DAT"),
        std::fs::read(src.join("MYSETTING.DAT")).unwrap(),
    )
    .unwrap();
    assert!(s.save(&dir).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
