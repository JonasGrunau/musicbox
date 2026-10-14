//! List performance on a large library (egui_kittest, headless): the oracle-a collection repeated
//! to `MB_PERF_TRACKS` tracks (default 50 000), 400 playlists in folders, one 20 000-track list.
//! Prints frame times for idle frames, scrolling, typing a search, sorting and switching lists.
//! `cargo test --release -p musicbox --test perf_lists -- --ignored --nocapture`

use egui::accesskit::Role;
use egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use mb_core::{DbId, LibrarySnapshot, PlaylistKind, PlaylistNode};
use musicbox::{App, Args};
use std::path::Path;
use std::time::{Duration, Instant};

fn big_library(fixture: &Path, n: usize) -> LibrarySnapshot {
    let paths = mb_masterdb::LibraryPaths::from_options_json(&fixture.join("options.json")).unwrap();
    let pass = mb_masterdb::derive_passphrase(&paths.dp).unwrap();
    let base = mb_masterdb::MasterDb::open_read_only(&fixture.join("rekordbox/master.db"), &pass).unwrap().load_snapshot(&paths).unwrap();
    let mut lib = LibrarySnapshot::default();
    for i in 0..n {
        let mut t = base.tracks[i % base.tracks.len()].clone();
        t.id = DbId(format!("{}", 10_000_000 + i));
        t.title = format!("{} {}", t.title, i / base.tracks.len());
        lib.tracks.push(t);
    }
    for f in 0..20 {
        let folder = DbId(format!("f{f}"));
        lib.playlists.push(PlaylistNode { id: folder.clone(), parent: None, name: format!("Folder {f}"), kind: PlaylistKind::Folder, seq: f });
        for p in 0..20 {
            let id = DbId(format!("p{f}-{p}"));
            lib.playlists.push(PlaylistNode { id: id.clone(), parent: Some(folder.clone()), name: format!("List {f}-{p}"), kind: PlaylistKind::Playlist, seq: p });
            let entries = (0..100).map(|k| lib.tracks[(f as usize * 977 + p as usize * 131 + k * 7) % n].id.clone()).collect();
            lib.playlist_entries.insert(id, entries);
        }
    }
    let big = DbId("big".into());
    lib.playlists.push(PlaylistNode { id: big.clone(), parent: None, name: "Big list".into(), kind: PlaylistKind::Playlist, seq: 99 });
    lib.playlist_entries.insert(big, lib.tracks.iter().step_by(2).take(20_000).map(|t| t.id.clone()).collect());
    lib
}

/// Runs `frames` frames and returns (mean, max) in ms.
fn frames(h: &mut Harness<'_, App>, frames: usize, mut before: impl FnMut(&mut Harness<'_, App>, usize)) -> (f64, f64) {
    let mut times = Vec::new();
    for i in 0..frames {
        before(h, i);
        let start = Instant::now();
        h.step();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    (mean, times.iter().copied().fold(0.0, f64::max))
}

/// (mean, max) of two frames as (sum, max).
fn total((mean, max): (f64, f64)) -> (f64, f64) {
    (mean * 2.0, max)
}

fn report(what: &str, (mean, max): (f64, f64)) {
    println!("{what:<40} mean {mean:7.2} ms   max {max:7.2} ms");
}

#[test]
#[ignore = "performance report; run with --release --ignored --nocapture"]
fn list_frame_times() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a");
    if !fixture.exists() {
        eprintln!("no fixtures, skipping");
        return;
    }
    let n = std::env::var("MB_PERF_TRACKS").ok().and_then(|v| v.parse().ok()).unwrap_or(50_000);
    let library = big_library(&fixture, n);
    let args = Args {
        options: Some(fixture.join("options.json")),
        db: Some(fixture.join("rekordbox/master.db")),
        analysis_root: Some(fixture.join("rekordbox/share")),
        no_volume_scan: true,
        ..Default::default()
    };
    let mut h = Harness::builder().with_size(Vec2::new(1440.0, 900.0)).build_eframe(|cc| {
        musicbox::install(&cc.egui_ctx);
        App::new(cc.egui_ctx.clone(), args)
    });
    let start = Instant::now();
    while h.query_all_by_label("oracle-small").next().is_none() {
        assert!(start.elapsed() < Duration::from_secs(60), "library load");
        h.step();
    }
    h.state_mut().library = library;
    h.state_mut().browser.invalidate();
    println!("{n} tracks, 400 playlists in 20 folders, a 20 000-track list");
    report("first frame (rows built)", frames(&mut h, 1, |_, _| {}));
    report("idle", frames(&mut h, 60, |_, _| {}));

    let table = egui::pos2(900.0, 600.0);
    h.hover_at(table);
    report("scrolling", frames(&mut h, 60, |h, _| {
        h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -240.0), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE });
    }));

    h.get_all_by_label("Folder 3").next().unwrap().click();
    h.run_steps(2);
    report("tree with an open folder", frames(&mut h, 30, |_, _| {}));

    let search = h.get_all_by_role(Role::TextInput).find(|n| n.accesskit_node().placeholder() == Some("Search")).unwrap();
    search.focus();
    h.step();
    let text = "chords";
    report("typing a search (per key)", frames(&mut h, text.len(), |h, i| {
        h.event(egui::Event::Text(text[i..=i].to_owned()));
    }));
    for _ in 0..text.len() {
        h.key_press(egui::Key::Backspace);
    }
    h.step();
    h.key_press(egui::Key::Escape);
    report("search cleared", frames(&mut h, 1, |_, _| {}));

    // A header click is read at the end of a frame and sorts in the next one: two frames each.
    h.get_all_by_label("Artist").next().unwrap().click();
    report("sort by artist (2 frames)", total(frames(&mut h, 2, |_, _| {})));
    h.get_all_by_label("Artist").next().unwrap().click();
    report("reverse sort (2 frames)", total(frames(&mut h, 2, |_, _| {})));
    h.get_all_by_label("Title").next().unwrap().click();
    report("sort by title (2 frames)", total(frames(&mut h, 2, |_, _| {})));

    h.get_all_by_label("Big list").next().unwrap().click();
    report("open the 20 000-track list (2 frames)", total(frames(&mut h, 2, |_, _| {})));
    report("idle in the big list", frames(&mut h, 30, |_, _| {}));
}
