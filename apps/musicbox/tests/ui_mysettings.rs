//! Headless UI test: "My Settings…" on a device. The device is a temp folder without settings
//! files; two player settings are changed and written. No fixtures, no OS input, no window.
//! Screenshots: set MB_UI_SHOTS=<dir> to save rendered frames for review.

use egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mb_usb::mysettings::{MySettings, field};
use mb_usb::settings::{SettingsFile, SettingsKind};
use musicbox::{App, Args};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn wait(h: &mut Harness<'_, App>, what: &str, secs: u64, cond: impl Fn(&Harness<'_, App>) -> bool) {
    let start = Instant::now();
    while !cond(h) {
        assert!(
            start.elapsed() < Duration::from_secs(secs),
            "timed out waiting for: {what}"
        );
        h.step();
        std::thread::sleep(Duration::from_millis(25));
    }
    h.run_steps(2);
}

fn shot(h: &mut Harness<'_, App>, name: &str) {
    let Ok(dir) = std::env::var("MB_UI_SHOTS") else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(img) = h.render() {
        let _ = img.save(Path::new(&dir).join(format!("{name}.png")));
    }
}

#[test]
fn edit_and_save_my_settings_on_a_device() {
    let root: PathBuf =
        std::env::temp_dir().join(format!("mb-ui-mysettings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let stick = root.join("MBSETTINGS");
    std::fs::create_dir_all(&stick).unwrap();

    let args = Args {
        options: Some(root.join("none/options.json")),
        db: Some(root.join("none/master.db")),
        devices: vec![stick.clone()],
        backup_root: Some(root.join("backups")),
        no_volume_scan: true,
        ..Args::default()
    };
    let mut h = Harness::builder()
        .with_size(Vec2::new(1280.0, 832.0))
        .build_eframe(|cc| {
            musicbox::install(&cc.egui_ctx);
            App::new(cc.egui_ctx.clone(), args)
        });
    wait(&mut h, "device", 30, |h| {
        h.query_by_label("MBSETTINGS").is_some()
    });

    h.get_by_label("MBSETTINGS").click_secondary();
    h.run_steps(3);
    h.get_by_label("My Settings…").click();
    h.run_steps(10);
    assert!(h.query_by_label("Settings stored on MBSETTINGS").is_some());
    assert!(h.query_by_label("TEMPO RANGE").is_some());
    shot(&mut h, "mysettings-1-open");

    // TEMPO RANGE: +/-10 (default) -> WIDE; AUTO CUE LEVEL: MEMORY CUE (default) -> -60dB.
    h.get_by_value("+/-10").click();
    h.run_steps(3);
    h.get_by_label("WIDE").click();
    h.run_steps(3);
    h.get_by_value("MEMORY CUE").click();
    h.run_steps(3);
    h.get_by_label("-60dB").click();
    h.run_steps(3);
    assert!(
        h.query_by_label("OK writes 1 file(s) to MBSETTINGS.")
            .is_some()
    );
    shot(&mut h, "mysettings-2-changed");
    h.get_by_label("OK").click();
    wait(&mut h, "saved", 10, |h| {
        h.query_by_label_contains("My Settings: wrote 1 file(s)")
            .is_some()
    });

    // Only MYSETTING.DAT was written, with rekordbox's header and a valid CRC.
    let pioneer = stick.join("PIONEER");
    let bytes = std::fs::read(pioneer.join("MYSETTING.DAT")).unwrap();
    let file = SettingsFile::parse(SettingsKind::MySetting, &bytes).expect("valid file");
    assert_eq!(
        (file.brand.as_str(), file.product.as_str()),
        ("PIONEER", "rekordbox")
    );
    assert_eq!(file.data[36], 0x83, "tempo range WIDE");
    assert_eq!(file.data[11], 0x84, "auto cue level -60dB");
    for kind in [
        SettingsKind::MySetting2,
        SettingsKind::DjmMySetting,
        SettingsKind::DevSetting,
    ] {
        assert!(
            !pioneer.join(kind.file_name()).exists(),
            "{} untouched",
            kind.file_name()
        );
    }
    let s = MySettings::load(&pioneer);
    let tempo = field("tempo_range").unwrap();
    assert_eq!(tempo.options[s.selected(tempo).unwrap()].1, "WIDE");
    let _ = std::fs::remove_dir_all(&root);
}
