//! `rbdiff xml <library_dir> <rekordbox.xml>`: compares a rekordbox.xml export with the library it
//! came from — master.db (read-only, via mb-masterdb) and the local ANLZ files (docs/re/xml.md).
//!
//! `<library_dir>` holds `options.json` and `rekordbox/` (master.db + share/) as captured in
//! fixtures. Every XML TRACK is paired with the djmdContent row whose ID equals its TrackID.
//! Per field the tool counts matches and prints each mismatch class with an example. Finally it
//! rebuilds the whole document from the library with `mb_xml::export` and compares bytes.
//!
//! Differences with a known cause (rows created after the XML file was written) are reported as
//! explained; the exit status counts only the unexplained ones.

use anyhow::{Context, Result};
use mb_anlz::AnlzFile;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use mb_xml::export::{self, GridBeat, KindLanguage};
use mb_xml::model::{fmt_bpm, fmt_seconds};
use mb_xml::{Document, PlaylistNode, PositionMark, Tempo, Track};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::SystemTime;

/// Columns the snapshot does not carry.
struct Row {
    file_type: i64,
    subtitle: String,
    color_id: u8,
    color_name: String,
    created_at: String,
}

struct Cue {
    kind: i64,
    in_ms: i64,
    out_ms: i64,
    comment: String,
}

#[derive(Default)]
struct Tally {
    same: usize,
    differ: usize,
    examples: Vec<String>,
}

#[derive(Default)]
struct Report {
    fields: BTreeMap<&'static str, Tally>,
    /// (class, explained) → examples
    classes: BTreeMap<(String, bool), Vec<String>>,
}

impl Report {
    fn check(
        &mut self,
        field: &'static str,
        track: &str,
        xml: impl std::fmt::Debug,
        db: impl std::fmt::Debug,
        same: bool,
    ) {
        let t = self.fields.entry(field).or_default();
        if same {
            t.same += 1;
        } else {
            t.differ += 1;
            if t.examples.len() < 3 {
                t.examples
                    .push(format!("{track}: xml={xml:?} library={db:?}"));
            }
        }
    }

    fn class(&mut self, class: impl Into<String>, explained: bool, example: String) {
        self.classes
            .entry((class.into(), explained))
            .or_default()
            .push(example);
    }
}

fn opt<T: Clone + Default>(v: &Option<T>) -> T {
    v.clone().unwrap_or_default()
}

pub fn run(library_dir: &Path, xml_path: &Path) -> Result<usize> {
    let mut paths = LibraryPaths::from_options_json(&library_dir.join("options.json"))?;
    paths.analysis_data_root = library_dir.join("rekordbox/share");
    let db = MasterDb::open_read_only(
        &library_dir.join("rekordbox/master.db"),
        &derive_passphrase(&paths.dp)?,
    )?;
    let snapshot = db.load_snapshot(&paths)?;
    let conn = db.connection();

    // djmdContent in table order (rowid), with the columns the snapshot lacks.
    let mut stmt = conn.prepare(
        "SELECT c.ID, c.FileType, c.Subtitle, c.ColorID, col.Commnt, c.created_at FROM djmdContent c \
         LEFT JOIN djmdColor col ON col.ID = c.ColorID AND col.rb_local_deleted = 0 \
         WHERE c.rb_local_deleted = 0 ORDER BY c.rowid",
    )?;
    let mut order = Vec::new();
    let mut rows: HashMap<String, Row> = HashMap::new();
    for r in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            Row {
                file_type: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                subtitle: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                color_id: r
                    .get::<_, Option<String>>(3)?
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0),
                color_name: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                created_at: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            },
        ))
    })? {
        let (id, row) = r?;
        order.push(id.clone());
        rows.insert(id, row);
    }
    let mut stmt = conn.prepare(
        "SELECT ContentID, Kind, InMsec, OutMsec, Comment FROM djmdCue WHERE rb_local_deleted = 0 ORDER BY ContentID, InMsec",
    )?;
    let mut cues: HashMap<String, Vec<Cue>> = HashMap::new();
    for r in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            Cue {
                kind: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                in_ms: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                out_ms: r.get::<_, Option<i64>>(3)?.unwrap_or(-1),
                comment: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            },
        ))
    })? {
        let (id, cue) = r?;
        cues.entry(id).or_default().push(cue);
    }

    let xml_bytes = std::fs::read_to_string(xml_path)
        .with_context(|| format!("reading {}", xml_path.display()))?;
    let doc = Document::parse(&xml_bytes)?;
    let exported_at = std::fs::metadata(xml_path)?
        .modified()
        .ok()
        .map(utc_timestamp);
    let language = doc
        .tracks()
        .iter()
        .find_map(|t| export::kind_from_label(t.kind.as_deref()?))
        .map_or(KindLanguage::English, |(_, l)| l);

    let tracks: HashMap<&str, &mb_core::Track> =
        snapshot.tracks.iter().map(|t| (t.id.as_str(), t)).collect();
    println!(
        "{}: {} tracks; master.db: {} tracks, {} cues; Kind language {language:?}; XML file written {}",
        xml_path.display(),
        doc.tracks().len(),
        tracks.len(),
        cues.values().map(Vec::len).sum::<usize>(),
        exported_at.as_deref().unwrap_or("?")
    );

    let mut report = Report::default();
    let mut beats_by_id: HashMap<String, Vec<GridBeat>> = HashMap::new();
    let mut tempo_stats = (0usize, 0usize, 0usize); // entries, tempo-change anchors, discontinuity anchors
    let mut marks_total = 0usize;
    let mut xml_ids = Vec::new();
    for x in doc.tracks() {
        let Some(xml_id) = x.track_id else {
            report.class("XML track without TrackID", false, format!("{:?}", x.name));
            continue;
        };
        let id = export::db_id(xml_id).to_string();
        xml_ids.push(id.clone());
        let label = format!("track {id} ({})", opt(&x.name));
        let (Some(t), Some(row)) = (tracks.get(id.as_str()), rows.get(&id)) else {
            report.class("track only in the XML", false, label);
            continue;
        };
        compare_track(&mut report, &label, x, t, row, language);

        // TEMPO vs PQTZ.
        let beats = load_beats(t.analysis_path.as_deref());
        let anchors = export::tempo_anchors(&beats);
        let expected = export::tempos_from_beat_grid(&beats, export::TempoMode::Changes);
        let key = |v: &[Tempo]| {
            v.iter()
                .map(|t| {
                    (
                        fmt_seconds(t.inizio),
                        fmt_bpm(t.bpm),
                        t.metro.clone(),
                        t.battito,
                    )
                })
                .collect::<Vec<_>>()
        };
        let same = key(&x.tempos) == key(&expected);
        report.check(
            "TEMPO (vs PQTZ reduction)",
            &label,
            x.tempos.len(),
            format!("{} of {} beats", expected.len(), beats.len()),
            same,
        );
        if !same {
            report.class(
                "TEMPO list differs from the PQTZ reduction",
                false,
                format!(
                    "{label}: xml {:?}, expected {:?}",
                    key(&x.tempos),
                    key(&expected)
                ),
            );
        }
        tempo_stats.0 += x.tempos.len();
        for (_, reason) in &anchors {
            match reason {
                export::AnchorReason::TempoChange => tempo_stats.1 += 1,
                export::AnchorReason::Discontinuity => tempo_stats.2 += 1,
            }
        }
        beats_by_id.insert(id.clone(), beats);

        // POSITION_MARK vs djmdCue (+ hot-cue colours from the ANLZ EXT).
        let expected_marks = expected_marks(
            cues.get(&id).map(Vec::as_slice).unwrap_or_default(),
            t.analysis_path.as_deref(),
        );
        let mark_key = |v: &[PositionMark]| {
            let mut k: Vec<_> = v
                .iter()
                .map(|m| {
                    (
                        m.num,
                        m.mark_type,
                        fmt_seconds(m.start),
                        m.end.map(fmt_seconds),
                        opt(&m.name),
                        m.rgb,
                    )
                })
                .collect();
            k.sort();
            k
        };
        marks_total += x.position_marks.len();
        let same = mark_key(&x.position_marks) == mark_key(&expected_marks);
        report.check(
            "POSITION_MARK (vs djmdCue)",
            &label,
            x.position_marks.len(),
            expected_marks.len(),
            same,
        );
        if !same {
            report.class(
                "POSITION_MARK set differs from djmdCue",
                false,
                format!(
                    "{label}: xml {:?}, expected {:?}",
                    mark_key(&x.position_marks),
                    mark_key(&expected_marks)
                ),
            );
        }
    }

    // Library rows the XML does not have.
    for id in order.iter().filter(|id| !xml_ids.contains(id)) {
        let row = &rows[id];
        let title = tracks.get(id.as_str()).map_or("", |t| t.title.as_str());
        let after = exported_at
            .as_deref()
            .is_some_and(|at| row.created_at.as_str() > at);
        let class = if after {
            "track only in master.db, created after the XML was written"
        } else {
            "track only in master.db"
        };
        report.class(
            class,
            after,
            format!("track {id} ({title}) created_at {}", row.created_at),
        );
    }

    // Collection order = djmdContent table order.
    let db_order: Vec<&String> = order.iter().filter(|id| xml_ids.contains(id)).collect();
    let same_order =
        db_order.iter().zip(&xml_ids).all(|(a, b)| *a == b) && db_order.len() == xml_ids.len();
    report.check(
        "COLLECTION order (djmdContent rowid)",
        "collection",
        xml_ids.len(),
        db_order.len(),
        same_order,
    );

    // Playlist tree.
    let expected_root = export::playlists_from_snapshot(&snapshot);
    match doc.playlists.as_deref() {
        Some([root]) => compare_nodes(&mut report, "", root, &expected_root),
        other => report.class(
            "PLAYLISTS is not a single ROOT node",
            false,
            format!("{other:?}"),
        ),
    }

    // Rebuild the document from the library, restricted to the exported tracks, and compare bytes.
    let product_version = doc
        .product
        .as_ref()
        .and_then(|p| p.version.clone())
        .unwrap_or_default();
    let mut rebuilt = Document::rekordbox(&product_version);
    let collection = rebuilt
        .collection
        .as_mut()
        .expect("rekordbox() has a collection");
    for id in order.iter().filter(|id| xml_ids.contains(id)) {
        let (t, row) = (tracks[id.as_str()], &rows[id]);
        let mut x = export::track_from_core(t, language, &beats_by_id[id]);
        x.kind = Some(export::kind_label(row.file_type, language).to_owned());
        x.grouping = Some(row.color_name.clone());
        x.mix = Some(row.subtitle.clone());
        x.colour = export::colour_rgb(row.color_id);
        x.position_marks = expected_marks(
            cues.get(id).map(Vec::as_slice).unwrap_or_default(),
            t.analysis_path.as_deref(),
        );
        collection.push(x);
    }
    rebuilt.playlists = Some(vec![expected_root]);
    let text = rebuilt.to_xml();
    let byte_exact = text == xml_bytes;

    // Output.
    println!("\n{:38} {:>6} {:>6}", "field", "match", "differ");
    for (name, t) in &report.fields {
        println!("{name:38} {:>6} {:>6}", t.same, t.differ);
    }
    println!(
        "\nTEMPO: {} entries; PQTZ reduction anchors: {} at tempo changes (incl. first beat), {} at grid discontinuities",
        tempo_stats.0, tempo_stats.1, tempo_stats.2
    );
    println!("POSITION_MARK: {marks_total} in the XML");
    let mut unexplained = 0;
    for (name, t) in &report.fields {
        if t.differ > 0 {
            unexplained += t.differ;
            println!("\n--- {name}: {} mismatches, e.g.", t.differ);
            for e in &t.examples {
                println!("  {e}");
            }
        }
    }
    for ((class, explained), examples) in &report.classes {
        if !explained {
            unexplained += examples.len();
        }
        println!(
            "\n--- {class} ({}): {}",
            if *explained {
                "explained"
            } else {
                "UNEXPLAINED"
            },
            examples.len()
        );
        for e in examples.iter().take(3) {
            println!("  {e}");
        }
    }
    println!(
        "\nrebuilt from master.db + ANLZ (the {} exported tracks): {}",
        xml_ids.len(),
        if byte_exact {
            "byte-exact".to_owned()
        } else {
            first_difference(&text, &xml_bytes)
        }
    );
    if !byte_exact {
        unexplained += 1;
    }
    println!("{unexplained} unexplained mismatches");
    Ok(unexplained)
}

fn compare_track(
    r: &mut Report,
    label: &str,
    x: &Track,
    t: &mb_core::Track,
    row: &Row,
    language: KindLanguage,
) {
    let s = |v: &Option<String>| v.clone().unwrap_or_default();
    let text_fields: [(&'static str, String, &str); 12] = [
        ("Name / Title", s(&x.name), &t.title),
        ("Artist", s(&x.artist), &t.artist),
        ("Composer", s(&x.composer), &t.composer),
        ("Album", s(&x.album), &t.album),
        ("Genre", s(&x.genre), &t.genre),
        ("Label", s(&x.label), &t.label),
        ("Remixer", s(&x.remixer), &t.remixer),
        ("Comments / Commnt", s(&x.comments), &t.comment),
        ("Tonality / djmdKey.ScaleName", s(&x.tonality), &t.key),
        ("DateAdded / StockDate", s(&x.date_added), &t.date_added),
        ("Mix / Subtitle", s(&x.mix), &row.subtitle),
        (
            "Grouping / djmdColor.Commnt",
            s(&x.grouping),
            &row.color_name,
        ),
    ];
    for (name, xv, dv) in text_fields {
        r.check(name, label, &xv, dv, xv == dv);
    }
    let kind = export::kind_label(row.file_type, language);
    r.check(
        "Kind / FileType",
        label,
        s(&x.kind),
        (row.file_type, kind),
        x.kind.as_deref() == Some(kind),
    );
    let num_fields: [(&'static str, Option<i64>, i64); 10] = [
        (
            "Size / FileSize",
            x.size,
            i64::from(t.file_size.unwrap_or(0) as i32),
        ),
        (
            "TotalTime / Length",
            x.total_time.map(i64::from),
            i64::from(t.length_seconds.unwrap_or(0)),
        ),
        (
            "DiscNumber / DiscNo",
            x.disc_number.map(i64::from),
            i64::from(t.disc_no.unwrap_or(0)),
        ),
        (
            "TrackNumber / TrackNo",
            x.track_number.map(i64::from),
            i64::from(t.track_no.unwrap_or(0)),
        ),
        (
            "Year / ReleaseYear",
            x.year.map(i64::from),
            i64::from(t.release_year.unwrap_or(0)),
        ),
        (
            "BitRate",
            x.bit_rate.map(i64::from),
            i64::from(t.bitrate.unwrap_or(0)),
        ),
        (
            "SampleRate",
            x.sample_rate.map(i64::from),
            i64::from(t.sample_rate.unwrap_or(0)),
        ),
        (
            "PlayCount / DJPlayCount",
            x.play_count.map(i64::from),
            i64::from(t.play_count),
        ),
        (
            "Rating (stars x 51)",
            x.rating.map(i64::from),
            i64::from(export::rating_to_xml(t.rating)),
        ),
        (
            "TrackID / ID",
            x.track_id,
            export::xml_id(t.id.as_str().parse().unwrap_or(0)),
        ),
    ];
    for (name, xv, dv) in num_fields {
        r.check(name, label, xv, dv, xv == Some(dv));
    }
    let bpm = t.bpm_x100.unwrap_or(0);
    r.check(
        "AverageBpm / BPM",
        label,
        x.average_bpm,
        bpm,
        x.average_bpm.map(|v| (v * 100.0).round() as u32) == Some(bpm)
            && x.average_bpm.map(fmt_bpm) == Some(fmt_bpm(export::bpm_from_x100(bpm))),
    );
    let path = t.path.to_string_lossy();
    let location = s(&x.location);
    r.check(
        "Location / FolderPath (decoded)",
        label,
        export::location_from_uri(&location),
        &path,
        export::location_from_uri(&location).as_deref() == Some(&*path),
    );
    r.check(
        "Location (encoding)",
        label,
        &location,
        export::location_to_uri(&path),
        location == export::location_to_uri(&path),
    );
    let colour = export::colour_rgb(row.color_id);
    r.check(
        "Colour / ColorID",
        label,
        x.colour,
        (row.color_id, colour),
        x.colour == colour,
    );
    r.check(
        "unknown attributes/children",
        label,
        &x.extra,
        "none",
        x.extra.is_empty(),
    );
}

fn load_beats(dat: Option<&Path>) -> Vec<GridBeat> {
    dat.and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| AnlzFile::parse(&bytes).ok())
        .and_then(|f| f.beat_grid())
        .map(|g| {
            g.beats
                .iter()
                .map(|b| GridBeat {
                    beat_in_bar: b.beat_in_bar,
                    tempo_x100: b.tempo,
                    time_ms: b.time_ms,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// POSITION_MARKs as rekordbox would write them for a track's djmdCue rows. Loop end: OutMsec > 0
/// (djmdCue stores -1 for "no end"; how rekordbox's cue loader maps that is open). Hot-cue colour:
/// the RGB of the ANLZ EXT PCP2 entry with the same hot-cue number, when it carries one.
fn expected_marks(cues: &[Cue], dat: Option<&Path>) -> Vec<PositionMark> {
    let ext = dat
        .and_then(|p| std::fs::read(p.with_extension("EXT")).ok())
        .and_then(|bytes| AnlzFile::parse(&bytes).ok());
    let hot_rgb = |num: i32| {
        ext.as_ref()?
            .cue_lists_ext()
            .iter()
            .flat_map(|l| l.entries.iter())
            .find(|e| i32::from(e.hot_cue) == num + 1)
            .and_then(|e| e.color())
            .map(|(_, rgb)| rgb)
    };
    cues.iter()
        .filter_map(|c| {
            let end = if c.out_ms > 0 { c.out_ms as u32 } else { 0 };
            let num = export::position_mark_num(c.kind)?;
            export::position_mark(c.kind, &c.comment, c.in_ms.max(0) as u32, end, hot_rgb(num))
        })
        .collect()
}

fn compare_nodes(r: &mut Report, path: &str, xml: &PlaylistNode, lib: &PlaylistNode) {
    let here = format!("{path}/{}", xml.name());
    r.check(
        "playlist node name",
        &here,
        xml.name(),
        lib.name(),
        xml.name() == lib.name(),
    );
    match (xml, lib) {
        (PlaylistNode::Folder { children: a, .. }, PlaylistNode::Folder { children: b, .. }) => {
            r.check(
                "folder child count",
                &here,
                a.len(),
                b.len(),
                a.len() == b.len(),
            );
            for (x, l) in a.iter().zip(b) {
                compare_nodes(r, &here, x, l);
            }
        }
        (
            PlaylistNode::Playlist {
                key_type,
                entries: a,
                ..
            },
            PlaylistNode::Playlist { entries: b, .. },
        ) => {
            r.check(
                "playlist KeyType",
                &here,
                key_type,
                "TrackID",
                *key_type == Some(mb_xml::KeyType::TrackId),
            );
            let (ka, kb): (Vec<&str>, Vec<&str>) = (
                a.iter().map(|e| e.key.as_str()).collect(),
                b.iter().map(|e| e.key.as_str()).collect(),
            );
            r.check(
                "playlist entries (djmdSongPlaylist by TrackNo)",
                &here,
                ka.len(),
                kb.len(),
                ka == kb,
            );
        }
        _ => r.check(
            "playlist node type",
            &here,
            "folder/playlist",
            "the other",
            false,
        ),
    }
}

/// `YYYY-MM-DD HH:MM:SS` in UTC, comparable with master.db's `created_at` text.
fn utc_timestamp(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn first_difference(ours: &str, theirs: &str) -> String {
    let at = ours
        .bytes()
        .zip(theirs.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or(ours.len().min(theirs.len()));
    let line = theirs[..at.min(theirs.len())].matches('\n').count() + 1;
    let snippet = |s: &str| {
        s.get(at.saturating_sub(40)..(at + 40).min(s.len()))
            .unwrap_or("")
            .to_owned()
    };
    format!(
        "DIFFERS at byte {at} (line {line}; {} vs {} bytes)\n  ours:      {:?}\n  rekordbox: {:?}",
        ours.len(),
        theirs.len(),
        snippet(ours),
        snippet(theirs)
    )
}
