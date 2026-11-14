//! List performance on a large library (egui_kittest, headless): the oracle-a collection repeated
//! to `MB_PERF_TRACKS` tracks (default 50 000), 400 playlists in folders, one 20 000-track list,
//! a smart list and 1 800 histories in 12 folders.
//! Prints frame times for idle frames, scrolling, typing a search, sorting and switching lists.
//! `component_frame_times` times single parts (the table's context menu on a large selection, a
//! device tree with 2 000 playlists, the deck's waveforms at 2 px per point) and
//! `thumbnail_memory` the image caches after scrolling past 300 tracks with their own artwork.
//! Frame times include tessellation (`Context::tessellate`), which `Harness::step` leaves out.
//! `cargo test --release -p musicbox --test perf_lists -- --ignored --nocapture --test-threads 1`

use egui::accesskit::Role;
use egui::Vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use mb_core::smartlist::{Condition, MATCH_ALL, SmartList, op};
use mb_core::{DbId, LibrarySnapshot, PlaylistKind, PlaylistNode};
use musicbox::app::{LibraryRef, List, Source};
use musicbox::devices::{Device, DeviceState, Devices};
use musicbox::{App, Args, browser, tree, waveform};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn load_fixture(fixture: &Path) -> LibrarySnapshot {
    let paths = mb_masterdb::LibraryPaths::from_options_json(&fixture.join("options.json")).unwrap();
    let pass = mb_masterdb::derive_passphrase(&paths.dp).unwrap();
    mb_masterdb::MasterDb::open_read_only(&fixture.join("rekordbox/master.db"), &pass).unwrap().load_snapshot(&paths).unwrap()
}

fn node(id: &str, parent: Option<&str>, name: String, kind: PlaylistKind, seq: i64) -> PlaylistNode {
    PlaylistNode { id: DbId(id.into()), parent: parent.map(|p| DbId(p.into())), name, kind, seq }
}

fn big_library(fixture: &Path, n: usize) -> LibrarySnapshot {
    let base = load_fixture(fixture);
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
    // A smart list with two text conditions, which fold every track's artist and title.
    lib.playlists.push(node("smart", None, "Smart list".into(), PlaylistKind::Smart, 100));
    let rules = vec![Condition::new("artist", op::CONTAINS, "", "e", ""), Condition::new("name", op::CONTAINS, "", "1", "")];
    lib.smart_lists.insert(DbId("smart".into()), SmartList::new("smart", MATCH_ALL, rules).to_xml());
    // 12 month folders of 150 histories each.
    for m in 0..12 {
        let folder = format!("h{m}");
        lib.histories.push(node(&folder, None, format!("History 2024-{:02}", m + 1), PlaylistKind::Folder, m));
        for d in 0..150 {
            let id = format!("h{m}-{d}");
            lib.histories.push(node(&id, Some(&folder), format!("HISTORY {m}-{d:03}"), PlaylistKind::Playlist, 150 - d));
            lib.history_entries.insert(DbId(id), (0..20).map(|k| lib.tracks[(m as usize * 1000 + d as usize * 20 + k) % n].id.clone()).collect());
        }
    }
    lib
}

/// Runs `frames` frames (UI pass and tessellation) and returns (mean, max) in ms.
fn frames<S>(h: &mut Harness<'_, S>, frames: usize, mut before: impl FnMut(&mut Harness<'_, S>, usize)) -> (f64, f64) {
    let mut times = Vec::new();
    for i in 0..frames {
        before(h, i);
        let start = Instant::now();
        h.step();
        let mut elapsed = start.elapsed();
        let (shapes, ppp) = (h.output().shapes.clone(), h.output().pixels_per_point);
        let start = Instant::now();
        std::hint::black_box(h.ctx.tessellate(shapes, ppp));
        elapsed += start.elapsed();
        times.push(elapsed.as_secs_f64() * 1000.0);
    }
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    (mean, times.iter().copied().fold(0.0, f64::max))
}

/// (mean, max) of two frames as (sum, max).
fn total((mean, max): (f64, f64)) -> (f64, f64) {
    (mean * 2.0, max)
}

fn report(what: &str, (mean, max): (f64, f64)) {
    println!("{what:<46} mean {mean:8.2} ms   max {max:8.2} ms");
}

/// Installs the app's fonts and style in the first pass of a component harness, which then
/// draws nothing (the fonts apply from the next pass on).
fn installed(ui: &egui::Ui) -> bool {
    if ui.ctx().cumulative_pass_nr() == 0 {
        musicbox::install(ui.ctx());
        return false;
    }
    true
}

fn fixture(name: &str) -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name);
    if path.exists() {
        Some(path)
    } else {
        eprintln!("no fixtures, skipping");
        None
    }
}

#[test]
#[ignore = "performance report; run with --release --ignored --nocapture"]
fn list_frame_times() {
    let Some(fixture) = fixture("oracle-a") else { return };
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
    h.state_mut().library = library.into();
    h.state_mut().browser.invalidate();
    println!("{n} tracks, 400 playlists in 20 folders, a 20 000-track list, a smart list, 1 800 histories");
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

    // Rows below the tree's fold are clicked through AccessKit, not at their off-screen position.
    h.get_all_by_label("Big list").next().unwrap().click_accesskit();
    report("open the 20 000-track list (2 frames)", total(frames(&mut h, 2, |_, _| {})));
    report("idle in the big list", frames(&mut h, 30, |_, _| {}));

    h.get_all_by_label("Smart list").next().unwrap().click_accesskit();
    report("open the smart list (2 frames)", total(frames(&mut h, 2, |_, _| {})));
    let search = h.get_all_by_role(Role::TextInput).find(|n| n.accesskit_node().placeholder() == Some("Search")).unwrap();
    search.focus();
    h.step();
    let text = "deep";
    report("typing a search in the smart list (per key)", frames(&mut h, text.len(), |h, i| {
        h.event(egui::Event::Text(text[i..=i].to_owned()));
    }));
    for _ in 0..text.len() {
        h.key_press(egui::Key::Backspace);
    }
    h.step();
    h.key_press(egui::Key::Escape);
    h.get_all_by_label("Artist").next().unwrap().click();
    report("sort the smart list (2 frames)", total(frames(&mut h, 2, |_, _| {})));
    println!("smart list: {} tracks", h.state().browser.visible_count());

    h.get_all_by_label("Histories").next().unwrap().click_accesskit();
    h.run_steps(2);
    h.get_all_by_label("History 2024-03").next().unwrap().click_accesskit();
    h.run_steps(2);
    report("tree with a 150-history folder open", frames(&mut h, 30, |_, _| {}));
}

/// The track table alone, editable, on `library`.
struct TableBench {
    library: LibrarySnapshot,
    source: Source,
    state: browser::State,
}

fn table_harness(library: LibrarySnapshot, size: Vec2) -> Harness<'static, TableBench> {
    let bench = TableBench { library, source: Source { library: LibraryRef::Local, list: List::Collection }, state: browser::State::default() };
    Harness::builder().with_size(size).build_ui_state(
        |ui, b: &mut TableBench| {
            if !installed(ui) {
                return;
            }
            let edit = browser::EditCtx { enabled: true, playlists: &b.library.playlists };
            browser::show(ui, &b.library, &b.source, &mut b.state, &edit);
        },
        bench,
    )
}

/// The deck's two waveforms, the playhead moving 1/120 s per frame while `playing`.
struct WaveBench {
    waves: waveform::TrackWaves,
    view: mb_analysis::view::ZoomView,
    marks: Vec<waveform::CueMark>,
    duration: u32,
    position: f64,
    playing: bool,
    style: waveform::Style,
}

/// A fixture track with analysis files and audio: its `ANLZ0000.DAT` and audio path.
fn analysed_track() -> Option<(PathBuf, PathBuf, u32)> {
    let fixture = fixture("oracle-a")?;
    load_fixture(&fixture).tracks.into_iter().find_map(|t| {
        let dat = t.analysis_path?;
        (dat.with_extension("EXT").is_file() && dat.with_extension("2EX").is_file() && t.path.is_file()).then(|| (dat, t.path, t.length_seconds.unwrap_or(0) * 1000))
    })
}

#[test]
#[ignore = "performance report; run with --release --ignored --nocapture"]
fn component_frame_times() {
    let Some(fixture) = fixture("oracle-a") else { return };
    let library = big_library(&fixture, 50_000);

    // Context menu of the table on 5 000 selected rows.
    let first_title = library.tracks[0].title.clone();
    let mut h = table_harness(library.clone(), Vec2::new(1200.0, 800.0));
    h.run_steps(2);
    h.state_mut().state.selection = (0..5_000).collect();
    h.state_mut().state.selected = Some((LibraryRef::Local, 0));
    h.run_steps(1);
    report("table, 5 000 selected, no menu", frames(&mut h, 30, |_, _| {}));
    h.get_all_by_label(&first_title).next().expect("first row").click_secondary();
    h.run_steps(2);
    assert!(h.query_by_label("Analyze Track").is_some(), "context menu open");
    report("table, 5 000 selected, context menu open", frames(&mut h, 10, |_, _| {}));
    drop(h);

    // A device with 2 000 playlists in 40 folders, all open.
    let mut device = LibrarySnapshot::default();
    for f in 0..40 {
        let folder = format!("df{f}");
        device.playlists.push(node(&folder, None, format!("DFolder {f}"), PlaylistKind::Folder, f));
        for p in 0..50 {
            let id = format!("dp{f}-{p}");
            device.playlists.push(node(&id, Some(&folder), format!("DList {f}-{p}"), PlaylistKind::Playlist, 50 - p));
            device.playlist_entries.insert(DbId(id), Vec::new());
        }
    }
    let mut devices = Devices::new(Vec::new(), false);
    devices.list.push(Device { name: "BIGSTICK".into(), root: PathBuf::from("/nonexistent"), state: DeviceState::Ready(Box::new(device)), write_refusal: None });
    let local = LibrarySnapshot::default();
    let mut source = Source { library: LibraryRef::Local, list: List::Collection };
    let mut h = Harness::builder().with_size(Vec2::new(260.0, 900.0)).build_ui(|ui| {
        if installed(ui) {
            tree::show(ui, &local, &devices, None, &mut source, false);
        }
    });
    h.run_steps(2);
    report("device tree, 40 folders closed", frames(&mut h, 30, |_, _| {}));
    for f in 0..40 {
        h.get_all_by_label(&format!("DFolder {f}")).next().unwrap().click_accesskit();
        h.step();
    }
    h.run_steps(2);
    report("device tree, 2 000 playlists open", frames(&mut h, 30, |_, _| {}));
    drop(h);

    // The local tree with 12 history folders, one open.
    let mut h = Harness::builder().with_size(Vec2::new(260.0, 900.0)).build_ui_state(
        |ui, lib: &mut LibrarySnapshot| {
            if !installed(ui) {
                return;
            }
            let mut source = Source { library: LibraryRef::Local, list: List::Collection };
            tree::show(ui, lib, &Devices::new(Vec::new(), false), None, &mut source, false);
        },
        library,
    );
    h.run_steps(2);
    h.get_all_by_label("Histories").next().unwrap().click_accesskit();
    h.run_steps(2);
    for m in 1..=12 {
        h.get_all_by_label(&format!("History 2024-{m:02}")).next().unwrap().click_accesskit();
        h.step();
    }
    h.run_steps(2);
    report("local tree, 1 800 histories open", frames(&mut h, 30, |_, _| {}));
    drop(h);

    // The deck's waveforms on a real track, at 2 px per point.
    let Some((dat, audio, length)) = analysed_track() else {
        eprintln!("no analysed fixture track with audio");
        return;
    };
    let waves = waveform::load(&dat).expect("analysis files");
    let mut view = mb_analysis::view::ZoomView::default();
    mb_analysis::view::build_zoom_view(&audio, |part| {
        view.ms.extend_from_slice(part);
        std::ops::ControlFlow::Continue(())
    })
    .expect("zoom view");
    let duration = length.max(view.ms.len() as u32);
    let marks = (0..8).map(|k| waveform::CueMark { ms: duration / 9 * (k + 1), end_ms: None, hot: Some((b'A' + k as u8) as char), color: waveform::hot_cue_color(None) }).collect();
    let bench = WaveBench { waves, view, marks, duration, position: 30_000.0, playing: true, style: waveform::Style::Rgb };
    let mut h = Harness::builder().with_size(Vec2::new(1240.0, 120.0)).with_pixels_per_point(2.0).build_ui_state(
        |ui, b: &mut WaveBench| {
            if !installed(ui) {
                return;
            }
            if b.playing {
                b.position += 1000.0 / 120.0;
            }
            ui.spacing_mut().item_spacing.y = 10.0;
            waveform::show_detail(ui, &b.waves, Some(&b.view), &b.marks, b.position, 4_000.0, 64.0, b.style);
            waveform::show_overview(ui, &b.waves, &b.marks, b.duration, b.style, Some(b.position), 32.0);
        },
        bench,
    );
    h.run_steps(2);
    report("waveforms, RGB, playing", frames(&mut h, 120, |_, _| {}));
    h.state_mut().style = waveform::Style::ThreeBand;
    h.run_steps(2);
    report("waveforms, 3-band, playing", frames(&mut h, 120, |_, _| {}));
    // `MB_UI_SHOTS=<dir>` saves both styles at one position, to compare renders across changes.
    if let Ok(dir) = std::env::var("MB_UI_SHOTS") {
        std::fs::create_dir_all(&dir).unwrap();
        for (style, name) in [(waveform::Style::Rgb, "rgb"), (waveform::Style::ThreeBand, "3band")] {
            let b = h.state_mut();
            (b.style, b.position, b.playing) = (style, 30_000.0, false);
            h.run_steps(3);
            h.render().unwrap().save(Path::new(&dir).join(format!("perf-waveforms-{name}.png"))).unwrap();
        }
    }
}

#[test]
#[ignore = "performance report; run with --release --ignored --nocapture"]
fn thumbnail_memory() {
    let Some(fixture) = fixture("oracle-a") else { return };
    let base = load_fixture(&fixture);
    let art = base.tracks.iter().find_map(|t| t.artwork_path.clone().filter(|p| p.is_file())).expect("a track with artwork");
    let dir = TempDir(std::env::temp_dir().join(format!("mb-perf-art-{}", std::process::id())));
    // 300 tracks, each with its own copy of the artwork folder (artwork.jpg, _s, _m).
    let mut library = LibrarySnapshot::default();
    for i in 0..300 {
        let folder = dir.0.join(format!("{i:03}"));
        std::fs::create_dir_all(&folder).unwrap();
        for name in ["artwork.jpg", "artwork_s.jpg", "artwork_m.jpg"] {
            let from = art.with_file_name(name);
            if from.is_file() {
                std::fs::copy(&from, folder.join(name)).unwrap();
            }
        }
        let mut t = base.tracks[i % base.tracks.len()].clone();
        t.id = DbId(format!("{}", 20_000_000 + i));
        t.title = format!("Art {i:03}");
        t.artwork_path = Some(folder.join("artwork.jpg"));
        library.tracks.push(t);
    }
    let mut h = table_harness(library, Vec2::new(1200.0, 800.0));
    h.run_steps(2);
    h.hover_at(egui::pos2(600.0, 400.0));
    let start = Instant::now();
    for _ in 0..60 {
        h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -160.0), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE });
        h.step();
        // Give the loader threads time, as a user scrolling at a normal pace would.
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(10));
            h.step();
        }
    }
    let scroll = start.elapsed();
    h.run_steps(5);
    let loaders = h.ctx.loaders();
    let mb = |b: usize| b as f64 / 1_048_576.0;
    let bytes: usize = loaders.bytes.lock().iter().map(|l| l.byte_size()).sum();
    let images: usize = loaders.image.lock().iter().map(|l| l.byte_size()).sum();
    let textures: usize = loaders.texture.lock().iter().map(|l| l.byte_size()).sum();
    println!(
        "scrolled 300 tracks with artwork in {:.1} s: file bytes {:.1} MB, decoded images {:.1} MB, textures {:.1} MB",
        scroll.as_secs_f64(),
        mb(bytes),
        mb(images),
        mb(textures)
    );
}

/// A directory removed when the test ends, also when it panics.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
