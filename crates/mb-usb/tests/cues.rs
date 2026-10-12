//! Device cue tags built from master.db (`docs/re/export.md` § 1.6 "Cues"):
//! 1. layout cross-check against reference-stick ANLZ files written by rekordbox (skipped
//!    without `fixtures/stick`);
//! 2. a copy of `fixtures/oracle-a` gets cues through `mb-library`, is exported to an empty
//!    folder, then re-exported after one more cue; the device PCOB/PCO2 are read back with
//!    `mb-anlz` (skipped without `fixtures/oracle-a`).

use mb_anlz::tags::{CueKind, CueList, CueListExt, CueType, NONE_U32};
use mb_anlz::{AnlzFile, Section, Tag};
use mb_library::{Command, CueSlot, DbId, Effect, Library, NewCue};
use mb_usb::cues::{SourceCue, apply_dat, apply_ext, device_cues};
use mb_usb::plan::FileOp;
use mb_usb::source::LibraryDir;
use mb_usb::{CancelToken, DeviceState, ExportOptions, PlanOptions, SelectionItem, SourceLibrary, execute, plan_export};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn read(p: &Path) -> AnlzFile {
    AnlzFile::parse(&std::fs::read(p).unwrap()).unwrap()
}

fn section(f: &AnlzFile, tag: Tag, kind: CueKind) -> Section {
    f.sections_with(tag).find(|s| u16::from_be_bytes([s.header[2], s.header[3]]) == kind.raw()).unwrap().clone()
}

fn hot(kind: i64, in_ms: i64, code: i64, micro: i64) -> SourceCue {
    SourceCue { kind, in_msec: in_ms, out_msec: -1, color: -1, color_table_index: code, cue_microsec: micro, ..Default::default() }
}

/// rekordbox's own cue tags, rebuilt from the rows its export would have read. The stick lists
/// are in reverse row order (`getExportCue` walks the cues backwards), so the rows are the
/// stick's PCO2 order reversed.
#[test]
fn reference_stick_cue_tags_are_reproduced() {
    let base = fixtures().join("stick/PIONEER/USBANLZ");
    if !base.exists() {
        eprintln!("no fixtures, skipping");
        return;
    }
    // Six hot cues A–F (7.x entries: status 1, 44-byte extension), sub-ms parts in CueMicrosec.
    let rows = [
        hot(1, 0, 43, 208),
        hot(3, 85_714, 23, 500),
        hot(2, 41_143, 8, 72),
        hot(7, 342_857, 38, 541),
        hot(6, 301_714, 19, 677),
        hot(5, 197_143, 60, 156),
    ];
    let set = base.join("P039/0002EE83/ANLZ0000");
    let (dat, ext) = (read(&set.with_extension("DAT")), read(&set.with_extension("EXT")));
    let cues = device_cues(&rows, 4);
    assert_eq!(cues.dat_hot.encode(), section(&dat, Tag::PCOB, CueKind::Hot));
    assert_eq!(cues.dat_memory.encode(), section(&dat, Tag::PCOB, CueKind::Memory));
    assert_eq!(cues.ext_hot.encode(), section(&ext, Tag::PCOB, CueKind::Hot));
    assert_eq!(cues.ext_memory.encode(), section(&ext, Tag::PCOB, CueKind::Memory));
    assert_eq!(cues.ext2_hot.encode(), section(&ext, Tag::PCO2, CueKind::Hot));
    assert_eq!(cues.ext2_memory.encode(), section(&ext, Tag::PCO2, CueKind::Memory));
    // Applying them to the stick's own files changes nothing.
    let (mut d, mut e) = (dat.clone(), ext.clone());
    apply_dat(&mut d, &cues);
    apply_ext(&mut e, &cues);
    assert_eq!((d, e), (dat, ext));

    // A hot loop with a beat-loop size (8/1) and in/out sub-ms parts.
    let set = base.join("P03D/000277E3/ANLZ0000");
    let looped = SourceCue {
        kind: 1,
        in_msec: 238_582,
        out_msec: 242_422,
        color: -1,
        color_table_index: 36,
        beat_loop_size: 8 << 16 | 1,
        cue_microsec: 494 | 494 << 16,
        ..Default::default()
    };
    let cues = device_cues(&[looped], 4);
    let (dat, ext) = (read(&set.with_extension("DAT")), read(&set.with_extension("EXT")));
    assert_eq!(cues.ext2_hot.encode(), section(&ext, Tag::PCO2, CueKind::Hot));
    assert_eq!(cues.dat_hot.encode(), section(&dat, Tag::PCOB, CueKind::Hot));
}

/// Every reference-stick set whose cues were written by a 7.x export (PCPT status 1, 44-byte
/// PCP2 extension) is rebuilt from rows derived from its own PCO2 lists.
#[test]
fn reference_stick_7x_cue_sets_round_trip() {
    let base = fixtures().join("stick/PIONEER/USBANLZ");
    if !base.exists() {
        eprintln!("no fixtures, skipping");
        return;
    }
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "EXT") {
                out.push(p);
            }
        }
    }
    let mut exts = Vec::new();
    walk(&base, &mut exts);
    let (mut checked, mut skipped) = (0, 0);
    for ext_path in exts {
        let dat_path = ext_path.with_extension("DAT");
        let (Ok(ext), Ok(dat)) = (AnlzFile::parse(&std::fs::read(&ext_path).unwrap()), AnlzFile::parse(&std::fs::read(&dat_path).unwrap())) else { continue };
        let pco2 = ext.cue_lists_ext();
        let entries: Vec<_> = pco2.iter().flat_map(|l| &l.entries).collect();
        let pcpt: Vec<_> = dat.cue_lists().into_iter().chain(ext.cue_lists()).flat_map(|l| l.entries).collect();
        if entries.is_empty() {
            continue;
        }
        if !(entries.iter().all(|e| e.unk_1d == 1 && e.ext.len() == 44) && pcpt.iter().all(|e| e.status == 1 && e.unk_28 == [0; 4])) {
            skipped += 1;
            continue;
        }
        // Rows in export order are the PCO2 hot list then memory list, each reversed per kind;
        // rebuild the row order by walking PCO2 hot and memory in reverse as one list.
        let mut rows: Vec<SourceCue> = Vec::new();
        for l in &pco2 {
            for e in &l.entries {
                let kind = match e.hot_cue {
                    0 => 0,
                    n @ 1..=3 => i64::from(n),
                    n => i64::from(n) + 1,
                };
                let ext_u64 = |at: usize| u64::from_be_bytes(e.ext[at..at + 8].try_into().unwrap());
                let ext_u32 = |at: usize| u32::from_be_bytes(e.ext[at..at + 4].try_into().unwrap());
                let seek = |a: u64, b: u64, n: u32| if n == 0 { String::new() } else { format!("{a},{b},{n}") };
                rows.push(SourceCue {
                    kind,
                    in_msec: i64::from(e.time),
                    out_msec: if e.loop_end == NONE_U32 { -1 } else { i64::from(e.loop_end) },
                    color: if e.color_index == 0 { -1 } else { i64::from(e.color_index) - 1 },
                    color_table_index: i64::from(e.ext[0]),
                    comment: e.comment.clone().unwrap_or_default(),
                    beat_loop_size: i64::from(e.loop_numerator) << 16 | i64::from(e.loop_denominator),
                    cue_microsec: i64::from(e.in_us) | i64::from(e.out_us) << 16,
                    in_point_seek_info: seek(ext_u64(4), ext_u64(20), ext_u32(36)),
                    out_point_seek_info: seek(ext_u64(12), ext_u64(28), ext_u32(40)),
                    ..Default::default()
                });
            }
        }
        rows.reverse();
        let cues = device_cues(&rows, 4);
        let (mut d, mut e) = (dat.clone(), ext.clone());
        apply_dat(&mut d, &cues);
        apply_ext(&mut e, &cues);
        assert_eq!(d, dat, "{}", dat_path.display());
        assert_eq!(e, ext, "{}", ext_path.display());
        checked += 1;
    }
    eprintln!("{checked} 7.x cue sets reproduced byte for byte, {skipped} older sets skipped");
    assert!(checked > 100, "{checked}");
}

/// A private copy of oracle-a's master.db (the share folder is only read).
struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("mb-usb-cues-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SMALL: &str = "1036096362";

fn add(lib: &mut Library, content: &DbId, cue: NewCue) -> String {
    match lib.execute(Command::AddCue { content: content.clone(), cue }).unwrap().effect {
        Effect::CueAdded { id } => id,
        other => panic!("unexpected {other:?}"),
    }
}

fn export(db: &Path, pass: &str, share: &Path, device: &Path, sel: &[SelectionItem]) -> mb_usb::ExportPlan {
    let library = LibraryDir::open_paths(db, pass, share.to_path_buf(), device.join("no-settings")).unwrap();
    let src = SourceLibrary::load(library.connection(), &library.analysis_root, sel).unwrap();
    let dev = DeviceState::load(device, Some(false)).unwrap();
    let plan = plan_export(&src, &dev, sel, &PlanOptions::default()).unwrap();
    let opts = ExportOptions { write_backup_info: false, check_free_space: false, ..ExportOptions::default() };
    execute(&plan, &dev, &opts, &CancelToken::new(), &mut |_| {}).unwrap();
    plan
}

/// The device `.DAT` the plan wrote for a track.
fn device_dat(plan: &mb_usb::ExportPlan, content: &str, root: &Path) -> Option<PathBuf> {
    plan.jobs.iter().flat_map(|j| &j.steps).filter(|s| s.content_id == content).flat_map(|s| &s.files).find_map(|f| match f {
        FileOp::WriteAnlz { dat, .. } => Some(root.join(dat.trim_start_matches('/'))),
        _ => None,
    })
}

#[test]
fn cues_from_master_db_reach_the_device_and_follow_a_reexport() {
    let fixture = fixtures().join("oracle-a");
    if !fixture.exists() {
        eprintln!("no fixtures, skipping");
        return;
    }
    let src = Tmp::new("src");
    let dev = Tmp::new("dev");
    let db = src.0.join("master.db");
    std::fs::copy(fixture.join("rekordbox/master.db"), &db).unwrap();
    let paths = mb_masterdb::LibraryPaths::from_options_json(&fixture.join("options.json")).unwrap();
    let pass = mb_masterdb::derive_passphrase(&paths.dp).unwrap();
    let share = fixture.join("rekordbox/share");

    let mut lib = Library::open_unguarded(&db, &pass).unwrap();
    let track: String = lib
        .connection()
        .query_row(
            "SELECT ContentID FROM djmdSongPlaylist WHERE PlaylistID = ?1 AND rb_local_deleted = 0 ORDER BY TrackNo LIMIT 1",
            [SMALL],
            |r| r.get(0),
        )
        .unwrap();
    let content = DbId(track.clone());
    let none = |slot, in_ms, out_ms| NewCue { slot, in_ms, out_ms, color: None, color_table_index: None, comment: String::new() };
    add(&mut lib, &content, NewCue { comment: "intro".into(), color: Some(4), ..none(CueSlot::Memory, 10_000, None) });
    add(&mut lib, &content, NewCue { color_table_index: Some(8), comment: "drop".into(), ..none(CueSlot::Hot(0), 20_000, None) });
    add(&mut lib, &content, none(CueSlot::Hot(3), 30_000, None));
    add(&mut lib, &content, none(CueSlot::Memory, 40_000, Some(44_000)));
    drop(lib);

    let plan = export(&db, &pass, &share, &dev.0, &[SelectionItem::Playlist(SMALL.into())]);
    let dat_path = device_dat(&plan, &track, &dev.0).expect("ANLZ written");
    let dat = read(&dat_path);
    let ext = read(&dat_path.with_extension("EXT"));
    let lists: Vec<CueList> = dat.cue_lists();
    let (hot, memory) = (&lists[0], &lists[1]);
    assert_eq!((hot.kind, memory.kind), (CueKind::Hot, CueKind::Memory));
    // .DAT: hot A only; memory cues in reverse row order (loop first), linked.
    assert_eq!(hot.entries.iter().map(|e| (e.hot_cue, e.time, e.status)).collect::<Vec<_>>(), vec![(1, 20_000, 1)]);
    assert_eq!(
        memory.entries.iter().map(|e| (e.time, e.loop_end, e.cue_type, e.prev, e.next)).collect::<Vec<_>>(),
        vec![(40_000, 44_000, CueType::Loop, 0xffff, 1), (10_000, NONE_U32, CueType::Point, 0, 0xffff)]
    );
    assert_eq!((memory.unk_14, memory.unk_16), (0, 1));
    // .EXT: PCOB hot holds D; PCO2 hot D then A, with colours and the comment.
    let ext_lists = ext.cue_lists();
    assert_eq!(ext_lists[0].entries.iter().map(|e| (e.hot_cue, e.time)).collect::<Vec<_>>(), vec![(4, 30_000)]);
    assert!(ext_lists[1].entries.is_empty());
    let pco2: Vec<CueListExt> = ext.cue_lists_ext();
    let h = &pco2[0].entries;
    assert_eq!(h.iter().map(|e| e.hot_cue).collect::<Vec<_>>(), vec![4, 1]);
    assert_eq!(&h[0].ext[..4], &[0, 0x4d, 0x00, 0xff], "D without a code: ColorFull default 60");
    assert_eq!(&h[1].ext[..4], &[8, 0x00, 0xc4, 0xff]);
    assert_eq!(h[1].comment.as_deref(), Some("drop"));
    let m = &pco2[1].entries;
    assert_eq!(m.iter().map(|e| (e.time, e.color_index, e.comment.clone())).collect::<Vec<_>>(), vec![(40_000, 0, None), (10_000, 5, Some("intro".into()))]);
    // Tracks without cues keep the empty lists (as in tests/oracle.rs).
    let other = plan.jobs.iter().flat_map(|j| &j.steps).find(|s| s.content_id != track && device_dat(&plan, &s.content_id, &dev.0).is_some()).unwrap();
    let other_path = device_dat(&plan, &other.content_id, &dev.0).unwrap();
    let other_bytes = std::fs::read(&other_path).unwrap();
    assert!(read(&other_path).cue_lists().iter().all(|l| l.entries.is_empty()));

    // Re-export after one more cue. An unchanged playlist is skipped as a whole (§ 1.7), so the
    // tracks are exported as tracks; CueUpdated changed for one of them, so only its ANLZ set
    // is rewritten (§ 4.2).
    let mut lib = Library::open_unguarded(&db, &pass).unwrap();
    add(&mut lib, &content, NewCue { color_table_index: Some(32), ..none(CueSlot::Hot(1), 50_000, None) });
    drop(lib);
    let sel = [SelectionItem::Tracks(vec![track.clone(), other.content_id.clone()])];
    let plan = export(&db, &pass, &share, &dev.0, &sel);
    let dat_path2 = device_dat(&plan, &track, &dev.0).expect("ANLZ rewritten on re-export");
    assert_eq!(dat_path2, dat_path, "same ANLZ set");
    let hot = &read(&dat_path2).cue_lists()[0];
    assert_eq!(hot.entries.iter().map(|e| (e.hot_cue, e.time)).collect::<Vec<_>>(), vec![(2, 50_000), (1, 20_000)]);
    assert_eq!(read(&dat_path2.with_extension("EXT")).cue_lists_ext()[0].entries.len(), 3);
    // The other track's set is rewritten too, because exportLibrary.db stores no update markers
    // (§ 4.2), but with the same bytes.
    assert_eq!(device_dat(&plan, &other.content_id, &dev.0).as_ref(), Some(&other_path));
    assert_eq!(std::fs::read(&other_path).unwrap(), other_bytes);
}
