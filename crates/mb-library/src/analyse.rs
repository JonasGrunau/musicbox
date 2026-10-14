//! Track analysis (`docs/re/ops/analyse-track.md`): musicbox analyses a track itself and writes
//! what rekordbox writes after "Analyze Track".
//!
//! Two steps, so the slow part never holds the database:
//! 1. [`analyse_track`] (pure, CPU-bound, no database): decodes the file, runs `mb-analysis`
//!    (waveforms, auto-gain, Normal-mode beat grid, key) and builds the local `.DAT`/`.EXT`/`.2EX`
//!    in rekordbox's tag order (`CMusicAnalyzeFile::write_pmai*`), plus the `djmdContent` values.
//! 2. [`Command::ApplyAnalysis`](crate::Command::ApplyAnalysis) writes the files and replays the
//!    traced statement sequence (start, completion, the agent's `contentFile` rows). Its inverse,
//!    [`Command::RestoreAnalysis`](crate::Command::RestoreAnalysis), puts the previous files and
//!    columns back.
//!
//! Sections musicbox does not compute (PVDI vocal, and PSSI phrase when the phrase pass is off)
//! are carried over from the previous analysis files when the track had them, and left out
//! otherwise, as `write_pmai_ext`/`write_pmai_ex2` leave out a section whose data is missing.

use crate::lookup::{NameTable, find_or_create, name_of};
use crate::{Command, DbId, Effect, Error, Library, Result, xml_sync};
use mb_anlz::tags::{
    BeatGrid, CueKind, CueList, CueListExt, ExtBeatGrid, Path as PathTag, Phrase, SeekPoint, SeekTable, SongStructure, TypedSection,
    Vbr, Wave3Band, WaveColorPreview, WaveColorScroll, WaveNormalize, WavePreview, WaveScroll,
};
use mb_anlz::{AnlzFile, FileHeader, Section, Tag};
use mb_masterdb::{CounterColumn, OrphanTable};
use mb_masterdb::rusqlite::OptionalExtension;
use mb_masterdb::rusqlite::types::Value;
use std::path::{Path, PathBuf};

/// Which analyses run (rekordbox's per-job flags from `rekordbox3.settings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisSettings {
    /// `AutoGain`: write the `djmdMixerParam` row.
    pub auto_gain: bool,
    /// `trackAnalysisSettingsSongStruct`: run the phrase pass, which moves the grid onto the
    /// first downbeat (`docs/re/analysis-phrase.md`). *Hypothesis:* this preference sets the
    /// thread flag +0x3908 that gates the shift in `CBeatAnalyzeThread::analyze`; a fresh
    /// install has it off (`fixtures/manifest.toml`, sandbox-initial).
    pub phrase: bool,
    /// `enableKeyAnalysis`: detect the key (`docs/re/analysis-key.md`) and write `KeyID`.
    pub key: bool,
    /// `HighPrecisionBeatAnalyze`: the high-precision beat grid in the given mode
    /// (`docs/re/analysis-beats-hp.md`); `None` for the Normal-mode grid.
    pub high_precision: Option<mb_analysis::hpbeat::HpMode>,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self { auto_gain: true, phrase: false, key: true, high_precision: None }
    }
}

impl AnalysisSettings {
    /// [`AnalysisSettings::high_precision`] from the preferences `HighPrecisionBeatAnalyze`
    /// (default 0) and `trackHiPrecisionBeatAnalysisMode` (default 2, Auto), as `execAnalyze`
    /// reads them.
    pub fn high_precision_from(enabled: Option<&str>, mode: Option<&str>) -> Option<mb_analysis::hpbeat::HpMode> {
        if enabled?.trim() != "1" {
            return None;
        }
        Some(mb_analysis::hpbeat::HpMode::from_pref(mode.and_then(|v| v.trim().parse().ok()).unwrap_or(2)))
    }
}

/// The result of [`analyse_track`]: the three local files and the `djmdContent` values.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackAnalysis {
    pub dat: AnlzFile,
    pub ext: AnlzFile,
    pub ex2: AnlzFile,
    /// BPM × 100 of the first beat, `None` when no beat was found.
    pub bpm_x100: Option<i64>,
    pub sample_rate: i64,
    pub bit_rate: i64,
    pub bit_depth: i64,
    /// Whole seconds.
    pub length: i64,
    /// `djmdMixerParam` (GainHigh, GainLow, PeakHigh, PeakLow), when auto-gain ran.
    pub mixer: Option<[i64; 4]>,
    /// The detected key as rekordbox names it (`djmdKey.ScaleName`, e.g. `Am`), when key
    /// analysis ran.
    pub key: Option<String>,
    /// The local artwork made from the file's embedded picture (`analyse-track.md` § Artwork),
    /// written only when the track has no artwork yet.
    pub artwork: Option<mb_usb::artwork::LocalArtworkSet>,
    /// High precision found no beats: rekordbox leaves the previous grid (PQTZ, PQT2) in place.
    pub keep_old_grid: bool,
    /// The phrase pass ran: `ext` holds its PSSI when it found phrases, and an earlier PSSI is
    /// not carried over.
    pub phrase_analysed: bool,
}

/// Analyses the audio file at `audio`. `file_name` is the name rekordbox puts into PPTH
/// (`?/<file name>`, `djmdContent.FileNameL`).
pub fn analyse_track(audio: &Path, file_name: &str, settings: &AnalysisSettings) -> Result<TrackAnalysis> {
    analyse_track_with(audio, file_name, settings, None)
}

/// [`analyse_track`] with the track's stored grid ([`stored_grid`]). When the high-precision
/// pass finds no beats, rekordbox keeps the stored grid and runs key and phrase analysis on it
/// (`getBeatListFromDatabase`); without it they run without a grid.
pub fn analyse_track_with(
    audio: &Path,
    file_name: &str,
    settings: &AnalysisSettings,
    stored: Option<&mb_analysis::beats::BeatGrid>,
) -> Result<TrackAnalysis> {
    let fail = |e: &dyn std::fmt::Display| Error::Invalid(format!("{}: {e}", audio.display()));
    let info = *mb_audio::open(audio).map_err(|e| fail(&e))?.info();
    let opts = mb_analysis::Options {
        loudness: settings.auto_gain,
        phrase: settings.phrase,
        key: settings.key,
        high_precision: settings.high_precision,
        ..mb_analysis::Options::default()
    };
    let a = mb_analysis::analyze_with(audio, opts, stored).map_err(|e| fail(&e))?;
    let waves = a.waveforms.unwrap_or_default();
    let grid = a.beats;

    let ppth = || TypedSection::Path(PathTag::new(mb_anlz::usb::local_ppth(file_name)));
    let beats: Vec<mb_anlz::tags::Beat> = grid
        .as_ref()
        .map(|g| {
            g.beats
                .iter()
                .map(|b| mb_anlz::tags::Beat { beat_in_bar: b.beat_in_bar, tempo: b.tempo, time_ms: b.time_ms })
                .collect()
        })
        .unwrap_or_default();

    // .DAT (`write_pmai`): PPTH PVBR PQTZ PWAV PWV2 PCOB(hot) PCOB(memory).
    let dat = AnlzFile::from_typed(
        FileHeader::default(),
        &[
            ppth(),
            TypedSection::Vbr(vbr(audio, &info)),
            TypedSection::BeatGrid(BeatGrid::new(beats.clone())),
            TypedSection::WavePreview(WavePreview { tag: Tag::PWAV, unk_0c: 0, unk_10: 1, unk_12: 0, data: waves.pwav.clone() }),
            TypedSection::WavePreview(WavePreview { tag: Tag::PWV2, unk_0c: 0, unk_10: 1, unk_12: 0, data: waves.pwv2.clone() }),
            TypedSection::CueList(CueList::empty(CueKind::Hot)),
            TypedSection::CueList(CueList::empty(CueKind::Memory)),
        ],
    );

    // .EXT (`write_pmai_ext`): PPTH PWV3 PCOB PCOB PCO2 PCO2 PQT2 PWV5 PWV4 [PVB2] [PSSI].
    let mut ext = vec![
        ppth(),
        TypedSection::WaveScroll(WaveScroll { unk_0c: 0, columns_per_second: 150, unk_16: 0, data: waves.pwv3.clone() }),
        TypedSection::CueList(CueList::empty(CueKind::Hot)),
        TypedSection::CueList(CueList::empty(CueKind::Memory)),
        TypedSection::CueListExt(CueListExt::empty(CueKind::Hot)),
        TypedSection::CueListExt(CueListExt::empty(CueKind::Memory)),
    ];
    if let Some(g) = &grid {
        let mut pqt2 = ExtBeatGrid::from_beats(&beats, g.beats.iter().map(|b| b.sub_ms).collect());
        pqt2.analysis_type = a.grid_type;
        ext.push(TypedSection::ExtBeatGrid(pqt2));
    }
    ext.push(TypedSection::WaveColorScroll(WaveColorScroll {
        unk_0c: 0,
        columns_per_second: 150,
        color_bits: 3,
        height_bits: 5,
        data: waves.pwv5.clone(),
    }));
    ext.push(TypedSection::WaveColorPreview(WaveColorPreview { unk_0c: 0, unk_14: 0, data: waves.pwv4.clone() }));
    if info.format == mb_audio::Format::Flac {
        let t = mb_audio::flac_seek::seek_table(audio).map_err(|e| fail(&e))?;
        ext.push(TypedSection::SeekTable(SeekTable {
            unk_0c: 0,
            unk_0e: 0,
            total_samples: t.total_samples,
            points: t
                .points
                .iter()
                .map(|p| SeekPoint { sample: p.sample, byte_offset: p.byte_offset, block_samples: p.block_samples })
                .collect(),
        }));
    }
    // PSSI: `set_pmai_ext_song_struct_data` @0x100f26a7c (`analysis-phrase.md` § 8).
    if let Some(r) = &a.phrase {
        ext.push(TypedSection::SongStructure(SongStructure {
            scrambled: false,
            unk_0c: 0,
            mood: r.mood as u16,
            unk_14: 0,
            unk_18: 0,
            end_beat: r.end_beat as u16,
            bank: r.bank as u8,
            unk_1d: r.unk_14 as u8,
            unk_1e: 0,
            unk_1f: 0,
            phrases: r.rows().into_iter().map(|words| Phrase { words }).collect(),
        }));
    }
    let ext = AnlzFile::from_typed(FileHeader::default(), &ext);

    // .2EX (`write_pmai_ex2`): PPTH PWV7 PWV6 PWVC [PVDI].
    let ex2 = AnlzFile::from_typed(
        FileHeader::default(),
        &[
            ppth(),
            TypedSection::Wave3Band(Wave3Band { tag: Tag::PWV7, unk_0c: 0, scroll: Some((150, 0)), data: waves.pwv7.clone() }),
            TypedSection::Wave3Band(Wave3Band { tag: Tag::PWV6, unk_0c: 0, scroll: None, data: waves.pwv6.clone() }),
            TypedSection::WaveNormalize(WaveNormalize { unk_0c: 0, coefficients: waves.pwvc.to_vec() }),
        ],
    );

    let (sample_rate, bit_rate, bit_depth) = stream_columns(audio, &info);
    // High precision without beats: the grid files stay, the BPM column becomes 0 (oracle-b's
    // 12 sweeps).
    let keep_old_grid = settings.high_precision.is_some() && grid.is_none();
    Ok(TrackAnalysis {
        dat,
        ext,
        ex2,
        bpm_x100: if keep_old_grid { Some(0) } else { grid.as_ref().filter(|g| !g.beats.is_empty()).map(|g| i64::from(g.bpm)) },
        sample_rate,
        bit_rate,
        bit_depth,
        // Whole seconds of the analysed samples `0 .. L − 1` (`decode.md` § 2): an exact 64 s file
        // stores 63 (verified on the probes).
        length: (info.total_samples.saturating_sub(1) / u64::from(info.sample_rate.max(1))) as i64,
        mixer: a.loudness.map(|l| {
            let (gh, gl, ph, pl) = l.mixer_param();
            [i64::from(gh), i64::from(gl), i64::from(ph), i64::from(pl)]
        }),
        key: a.key.map(|k| mb_analysis::key::scale_name(k).to_owned()),
        artwork: crate::tags::embedded_picture(audio).and_then(|p| mb_usb::artwork::local_set(&p).ok()),
        keep_old_grid,
        phrase_analysed: settings.phrase,
    })
}

/// PVBR (`write_pmai_vbr`): always the 400-entry table plus the sample count. MP3s carry the
/// decoded sample count, and the frame index when the file has a Xing header
/// (`docs/re/decode.md` § MP3); every other format has zeros.
fn vbr(audio: &Path, info: &mb_audio::StreamInfo) -> Vbr {
    let mut index = vec![0u32; 400];
    let mut count = 0u32;
    if info.format == mb_audio::Format::Mp3 {
        count = info.total_samples as u32;
        if let Ok(data) = std::fs::read(audio)
            && let Some(map) = mb_audio::mp3::scan(&data)
            && has_xing(&data)
        {
            let frames: Vec<_> = map.frames.iter().copied().filter(|f| f.pos >= map.music_start as i32).collect();
            index = mb_audio::mp3::pvbr_index(&frames);
        }
    }
    Vbr { unk_0c: 0, unk_0e: 0, index, sample_count: Some(count) }
}

/// A `Xing` header in the first frame (after an ID3v2 tag): the files whose PVBR index rekordbox
/// fills (`mb-audio` `tests/oracle.rs` `pvbr_index_matches_rekordbox`, 37/37 MP3s).
fn has_xing(data: &[u8]) -> bool {
    let start = if data.starts_with(b"ID3") && data.len() >= 10 {
        10 + data[6..10].iter().fold(0usize, |a, &b| (a << 7) | usize::from(b & 0x7f))
    } else {
        0
    };
    data.get(start..(start + 2048).min(data.len())).is_some_and(|w| w.windows(4).any(|w| w == b"Xing"))
}

/// `SampleRate`, `BitRate`, `BitDepth` as the analysis writes them (`analyse-track.md` § Values):
/// the file's rate; PCM bit rate = rate × bits × channels / 1000, MP3 the first frame's rate,
/// FLAC/ALAC/AAC 0; 16 bits for lossy formats.
fn stream_columns(audio: &Path, info: &mb_audio::StreamInfo) -> (i64, i64, i64) {
    use mb_audio::Format;
    let rate = i64::from(info.file_rate);
    let bits = i64::from(info.bits);
    match info.format {
        Format::Wav | Format::Aiff => (rate, rate * bits * i64::from(info.channels) / 1000, bits),
        Format::Mp3 => {
            let kbps = std::fs::read(audio).ok().and_then(|d| mb_audio::mp3::scan(&d)).map_or(0, |m| i64::from(m.first_bitrate) / 1000);
            (rate, kbps, 16)
        }
        Format::Aac => (rate, 0, 16),
        Format::Flac | Format::Alac => (rate, 0, bits),
    }
}

// ---- writing ---------------------------------------------------------------------------------

/// The `djmdContent` columns an analysis writes, as they were before (for undo).
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisColumns {
    pub analysis_data_path: Value,
    pub bpm: Value,
    pub sample_rate: Value,
    pub bit_rate: Value,
    pub bit_depth: Value,
    pub length: Value,
    pub analysed: Value,
    pub content_link: Value,
}

/// Everything [`Command::RestoreAnalysis`] needs to undo an analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisSnapshot {
    pub content: DbId,
    /// Absolute paths of `.DAT`, `.EXT`, `.2EX` and their previous bytes (`None` = did not exist).
    pub files: Vec<(PathBuf, Option<Vec<u8>>)>,
    pub columns: AnalysisColumns,
    /// `djmdMixerParam.ID` inserted by the analysis.
    pub mixer_inserted: Option<String>,
    /// `contentFile` rows: (ID, previous (Hash, Size), or `None` when the analysis inserted it).
    pub content_files: Vec<(String, Option<(String, i64)>)>,
    /// When the analysis wrote `KeyID`: the previous `KeyID` and the `ScaleName` it pointed to.
    pub key: Option<(Value, Option<String>)>,
    /// When the analysis added artwork: the previous `ImagePath` (its files are in `files`, its
    /// `contentFile` row in `content_files`).
    pub image_path: Option<Value>,
}

/// `Analysed` states (`analyse-track.md` § Values): bit 0x01 done, 0x02 in progress, 0x80 the
/// analysis lock (`modAnalyzeStatus` @0x1017e37fc ORs it in; `analyze_complete` @0x101494740
/// skips locked tracks).
const ANALYSED_DONE: i64 = 0x69;
const ANALYSED_LOCK: i64 = 0x80;
/// `ContentLink` bits (`analyze_complete`): 0x50 while queued; completion clears the
/// `modAnalyzeStatus` mask 0x8670 and sets 0xC0600; 0x10000 = phrase
/// (`setAnalyzedSongStructToContentLink`), 0x100000 = vocal, 0x200000 = embeddings.
const LINK_QUEUED: i64 = 0x50;
const LINK_DONE: i64 = 0xC0600;
const LINK_STATUS_MASK: i64 = 0x8670;
const LINK_PHRASE: i64 = 0x10000;
const LINK_VOCAL: i64 = 0x100000;

fn int(v: &Value) -> i64 {
    match v {
        Value::Integer(n) => *n,
        Value::Real(f) => *f as i64,
        Value::Text(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn io(path: &Path, e: std::io::Error) -> Error {
    Error::Io { path: path.display().to_string(), source: e }
}

/// Writes next to the file and renames it into place.
fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    }
    let tmp = path.with_extension("musicbox-tmp");
    std::fs::write(&tmp, bytes).map_err(|e| io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io(path, e))
}

/// The grid [`analyse_track_with`] falls back to: the PQTZ of the track's current `.DAT` and its
/// `djmdContent.BPM` (`bpm_x100`). Where the column is missing or 0 (left so by an earlier
/// high-precision pass without beats), beat 0's tempo stands in (hypothesis: oracle-b's sweeps
/// agree with both). Reads the file only.
pub fn stored_grid(dat: &Path, bpm_x100: Option<i64>) -> Option<mb_analysis::beats::BeatGrid> {
    let f = AnlzFile::parse(&std::fs::read(dat).ok()?).ok()?;
    let q = f.beat_grid()?;
    let bpm = match bpm_x100.unwrap_or(0) {
        v if v > 0 => i32::try_from(v).ok()?,
        _ => i32::from(q.beats.first()?.tempo),
    };
    let beats = q
        .beats
        .iter()
        .map(|b| mb_analysis::beats::Beat { beat_in_bar: b.beat_in_bar, tempo: b.tempo, time_ms: b.time_ms, sub_ms: 0 })
        .collect();
    Some(mb_analysis::beats::BeatGrid { bpm, beats })
}

/// The first section with `tag` of the file at `path`, if any.
fn old_section(path: &Path, tag: Tag) -> Option<Section> {
    let f = AnlzFile::parse(&std::fs::read(path).ok()?).ok()?;
    f.sections_with(tag).next().cloned()
}

/// The previous PQTZ and PQT2 in place of the new (empty) grid, as rekordbox leaves them when
/// the high-precision pass finds no beats (`analysis-beats-hp.md` § 6).
fn keep_old_grid(paths: &[PathBuf; 3], dat: &mut AnlzFile, ext: &mut AnlzFile) {
    if let (Some(old), Some(s)) = (old_section(&paths[0], Tag::PQTZ), dat.sections.iter_mut().find(|s| s.tag == Tag::PQTZ)) {
        *s = old;
    }
    if let Some(old) = old_section(&paths[1], Tag::PQT2) {
        let at = ext.sections.iter().position(|s| s.tag == Tag::PWV5).unwrap_or(ext.sections.len());
        ext.sections.insert(at, old);
    }
}

/// `ID` of a `contentFile` row: the content UUID, `_`, the path with `/` percent-encoded
/// (`common.md` § Agent-written `contentFile` rows).
fn content_file_id(uuid: &str, path: &str) -> String {
    format!("{uuid}_{}", path.replace('%', "%25").replace('/', "%2F"))
}

fn md5_hex(bytes: &[u8]) -> String {
    use md5::Digest;
    md5::Md5::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The agent's `contentFile` row for one analysis or artwork file: inserted when missing, new
/// hash and size when the file changed, untouched when equal (`common.md`). Returns the row ID
/// and its previous (hash, size) for undo.
fn upsert_content_file(
    tx: &mb_masterdb::WriteTxn<'_>,
    uuid: &str,
    content: &Value,
    root: &Path,
    path: &str,
    bytes: &[u8],
) -> Result<(String, Option<(String, i64)>)> {
    let id = content_file_id(uuid, path);
    let (hash, size) = (md5_hex(bytes), bytes.len() as i64);
    let existing: Option<(String, i64)> = tx
        .connection()
        .query_row("SELECT Hash, Size FROM contentFile WHERE ID = ?1 AND rb_local_deleted = 0", [id.as_str()], |r| {
            Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<i64>>(1)?.unwrap_or(0)))
        })
        .optional()?;
    match &existing {
        None => {
            tx.insert(
                "contentFile",
                &[
                    ("ID", Value::Text(id.clone())),
                    ("ContentID", content.clone()),
                    ("Path", Value::Text(path.to_owned())),
                    ("Hash", Value::Text(hash)),
                    ("Size", Value::Integer(size)),
                    ("rb_local_path", Value::Text(root.join(path.trim_start_matches('/')).display().to_string())),
                    ("rb_file_hash_dirty", Value::Integer(0)),
                    ("rb_local_file_status", Value::Integer(0)),
                    ("rb_in_progress", Value::Integer(0)),
                    ("rb_process_type", Value::Integer(0)),
                    ("rb_priority", Value::Integer(50)),
                    ("rb_file_size_dirty", Value::Integer(0)),
                    ("UUID", Value::Text(mb_masterdb::write::new_uuid())),
                ],
            )?;
        }
        // A changed file gets its new hash and size (*hypothesis*: what the agent does then is
        // open).
        Some((h, s)) if *h != hash || *s != size => {
            tx.update("contentFile", "ID", Value::Text(id.clone()), &[("Hash", Value::Text(hash)), ("Size", Value::Integer(size))])?;
        }
        Some(_) => {}
    }
    Ok((id, existing))
}

pub(crate) fn apply(lib: &mut Library, content: &DbId, analysis: &TrackAnalysis) -> Result<(Effect, Option<Command>)> {
    let row = lib
        .connection()
        .query_row(
            "SELECT UUID, AnalysisDataPath, BPM, SampleRate, BitRate, BitDepth, Length, Analysed, ContentLink, KeyID \
             FROM djmdContent WHERE ID = ?1 AND rb_local_deleted = 0",
            [content.as_str()],
            |r| Ok((r.get::<_, Option<String>>(0)?, (1..10).map(|i| r.get::<_, Value>(i)).collect::<std::result::Result<Vec<_>, _>>()?)),
        )
        .optional()?;
    let (uuid, cols) = row.ok_or_else(|| Error::NotFound { what: "track", id: content.0.clone() })?;
    let old_key_id = cols[8].clone();
    let uuid = uuid.filter(|u| !u.is_empty()).ok_or_else(|| Error::Invalid("track has no UUID".into()))?;
    let old = AnalysisColumns {
        analysis_data_path: cols[0].clone(),
        bpm: cols[1].clone(),
        sample_rate: cols[2].clone(),
        bit_rate: cols[3].clone(),
        bit_depth: cols[4].clone(),
        length: cols[5].clone(),
        analysed: cols[6].clone(),
        content_link: cols[7].clone(),
    };
    let analysed = int(&old.analysed);
    if analysed & ANALYSED_LOCK != 0 {
        return Err(Error::Invalid("the track's analysis is locked".into()));
    }
    let root = lib.analysis_root.clone().ok_or_else(|| Error::Invalid("analysis folder unknown".into()))?;
    // `AnalysisDataPath` = `/PIONEER/USBANLZ/<u[0:3]>/<u[3:]>/ANLZ0000.DAT`, rewritten on every pass.
    let rel = format!("/PIONEER/USBANLZ/{}/ANLZ0000.DAT", mb_anlz::usb::local_anlz_folder(&uuid));
    let dat_path = root.join(rel.trim_start_matches('/'));
    let paths = [dat_path.clone(), dat_path.with_extension("EXT"), dat_path.with_extension("2EX")];

    // Sections musicbox does not compute are kept from the previous files.
    let mut ext = analysis.ext.clone();
    let mut ex2 = analysis.ex2.clone();
    let own_phrase = ext.sections_with(Tag::PSSI).next().is_some();
    let kept_phrase = !analysis.phrase_analysed
        && old_section(&paths[1], Tag::PSSI).inspect(|s| ext.sections.push(s.clone())).is_some();
    let kept_vocal = old_section(&paths[2], Tag::PVDI).inspect(|s| ex2.sections.push(s.clone())).is_some();
    let mut dat = analysis.dat.clone();
    if analysis.keep_old_grid {
        keep_old_grid(&paths, &mut dat, &mut ext);
    }
    let new_bytes = [dat.to_bytes(), ext.to_bytes(), ex2.to_bytes()];

    // Copy-on-write backup and the undo snapshot of the files.
    let previous: Vec<(PathBuf, Option<Vec<u8>>)> = paths.iter().map(|p| (p.clone(), std::fs::read(p).ok())).collect();
    if let Some(dir) = &lib.anlz_backup {
        let target = dir.join("anlz").join(Path::new(rel.trim_start_matches('/')).parent().unwrap_or(Path::new("")));
        for (p, bytes) in &previous {
            if let (Some(bytes), Some(name)) = (bytes, p.file_name()) {
                let dst = target.join(name);
                if !dst.exists() {
                    std::fs::create_dir_all(&target).map_err(|e| io(&target, e))?;
                    std::fs::write(&dst, bytes).map_err(|e| io(&dst, e))?;
                }
            }
        }
    }

    let key = || Value::Text(content.0.clone());
    let col = |tx: &mb_masterdb::WriteTxn<'_>, name: &str, v: Value| tx.update("djmdContent", "ID", key(), &[(name, v)]);
    let link = int(&old.content_link);
    // Artwork (first analysis only): the embedded picture becomes the local set when the track
    // has none (`add_track_artwork` @0x100c45e4c), before the analysis starts.
    let old_image: Value = lib.connection().query_row("SELECT ImagePath FROM djmdContent WHERE ID = ?1", [content.as_str()], |r| r.get(0))?;
    let artwork = analysis.artwork.as_ref().filter(|_| matches!(&old_image, Value::Null) || matches!(&old_image, Value::Text(t) if t.is_empty()));
    let art_rel = format!("/PIONEER/Artwork/{}/artwork.jpg", mb_anlz::usb::local_anlz_folder(&uuid));
    let art_files: Vec<(PathBuf, &[u8])> = artwork.map_or_else(Vec::new, |a| {
        let base = root.join(art_rel.trim_start_matches('/'));
        vec![
            (base.clone(), a.original.as_slice()),
            (base.with_file_name("artwork_s.jpg"), a.small.as_slice()),
            (base.with_file_name("artwork_m.jpg"), a.medium.as_slice()),
        ]
    });
    let mut previous = previous;
    let tx = lib.writer.transaction()?;
    let mut content_files = Vec::new();
    if let Some(a) = artwork {
        for (p, bytes) in &art_files {
            previous.push((p.clone(), std::fs::read(p).ok()));
            write_file(p, bytes)?;
        }
        col(&tx, "ImagePath", Value::Text(art_rel.clone()))?;
        content_files.push(upsert_content_file(&tx, &uuid, &key(), &root, &art_rel, &a.original)?);
    }
    // A. Request (only a re-analysis): done bit cleared, queued.
    let mut state = analysed;
    if analysed & 0x01 != 0 {
        state = analysed & !0x01;
        col(&tx, "Analysed", Value::Integer(state))?;
        col(&tx, "ContentLink", Value::Integer(link | LINK_QUEUED))?;
    }
    // B. Start: 0x4A on the first pass (0 → 74), 0x6A afterwards (104 → 106); queued link bits.
    col(&tx, "Analysed", Value::Integer((state & 0x20) | 0x4A))?;
    col(&tx, "ContentLink", Value::Integer(link | LINK_QUEUED))?;
    // The files (the transaction is still open, so a failed write rolls the columns back).
    for (p, bytes) in paths.iter().zip(&new_bytes) {
        write_file(p, bytes)?;
    }
    // C. Completion.
    col(&tx, "AnalysisDataPath", Value::Text(rel.clone()))?;
    tx.increment_counter(content.as_str(), CounterColumn::AnalysisUpdated)?;
    col(&tx, "BPM", analysis.bpm_x100.map_or(old.bpm.clone(), Value::Integer))?;
    col(&tx, "SampleRate", Value::Integer(analysis.sample_rate))?;
    col(&tx, "BitRate", Value::Integer(analysis.bit_rate))?;
    col(&tx, "BitDepth", Value::Integer(analysis.bit_depth))?;
    col(&tx, "Length", Value::Integer(analysis.length))?;
    tx.increment_counter(content.as_str(), CounterColumn::TrackInfoUpdated)?;
    let mut mixer_inserted = None;
    if let Some([gh, gl, ph, pl]) = analysis.mixer {
        let exists = tx
            .connection()
            .query_row("SELECT 1 FROM djmdMixerParam WHERE rb_local_deleted = 0 AND ContentID = ?1", [content.as_str()], |_| Ok(()))
            .optional()?
            .is_some();
        if !exists {
            let id = mb_masterdb::write::new_uuid();
            tx.insert(
                "djmdMixerParam",
                &[
                    ("ID", Value::Text(id.clone())),
                    ("ContentID", key()),
                    ("GainHigh", Value::Integer(gh)),
                    ("GainLow", Value::Integer(gl)),
                    ("PeakHigh", Value::Integer(ph)),
                    ("PeakLow", Value::Integer(pl)),
                    ("UUID", Value::Text(mb_masterdb::write::new_uuid())),
                ],
            )?;
            mixer_inserted = Some(id);
        }
    }
    // The key: the `djmdKey` row by `ScaleName` (inserted when missing), `KeyID` written on every
    // pass, key rows no track uses any more removed (e.g. the tag key the analysis replaced),
    // then `TrackInfoUpdated`.
    let mut key_snapshot = None;
    if let Some(name) = &analysis.key {
        let old_name = match &old_key_id {
            Value::Text(id) if !id.is_empty() => name_of(&tx, "djmdKey", id)?,
            Value::Integer(n) => name_of(&tx, "djmdKey", &n.to_string())?,
            _ => None,
        };
        let id = find_or_create(&tx, NameTable::Key, name)?;
        col(&tx, "KeyID", Value::Text(id))?;
        tx.cleanup_orphan_table(OrphanTable::Key)?;
        tx.increment_counter(content.as_str(), CounterColumn::TrackInfoUpdated)?;
        key_snapshot = Some((old_key_id.clone(), old_name));
    }
    // Phrase and vocal: only what the files carry (phrase: ours, or kept from an earlier
    // analysis; `setAnalyzedSongStructToContentLink` sets 0x10000).
    let mut final_link = ((link | LINK_QUEUED) & !LINK_STATUS_MASK) | 0xE | LINK_DONE;
    final_link &= !(LINK_PHRASE | LINK_VOCAL);
    if kept_phrase {
        final_link |= link & LINK_PHRASE;
    }
    if own_phrase {
        final_link |= LINK_PHRASE;
    }
    if kept_vocal {
        final_link |= link & LINK_VOCAL;
    }
    col(&tx, "Analysed", Value::Integer(ANALYSED_DONE))?;
    col(&tx, "ContentLink", Value::Integer(final_link))?;
    // D. The agent's `contentFile` rows (.2EX, .DAT, .EXT), when musicbox writes in its place.
    for (ext_name, bytes) in [("2EX", &new_bytes[2]), ("DAT", &new_bytes[0]), ("EXT", &new_bytes[1])] {
        let path = rel.replace(".DAT", &format!(".{ext_name}"));
        content_files.push(upsert_content_file(&tx, &uuid, &key(), &root, &path, bytes)?);
    }
    tx.commit()?;
    sync_playlists(lib, content)?;

    let inverse = Command::RestoreAnalysis {
        snapshot: Box::new(AnalysisSnapshot {
            content: content.clone(),
            files: previous,
            columns: old,
            mixer_inserted,
            content_files,
            key: key_snapshot,
            image_path: artwork.map(|_| old_image.clone()),
        }),
    };
    Ok((Effect::Analysed { content: content.clone() }, Some(inverse)))
}

/// `masterPlaylists6.xml` timestamps of the lists that contain the track (completion step).
fn sync_playlists(lib: &mut Library, content: &DbId) -> Result<()> {
    let playlists: Vec<String> = {
        let mut stmt = lib
            .connection()
            .prepare("SELECT DISTINCT PlaylistID FROM djmdSongPlaylist WHERE rb_local_deleted = 0 AND ContentID = ?1")?;
        stmt.query_map([content.as_str()], |r| r.get(0))?.collect::<mb_masterdb::rusqlite::Result<_>>()?
    };
    if !playlists.is_empty() {
        xml_sync::sync(lib, &playlists)?;
    }
    Ok(())
}

pub(crate) fn restore(lib: &mut Library, s: &AnalysisSnapshot) -> Result<(Effect, Option<Command>)> {
    let key = || Value::Text(s.content.0.clone());
    let tx = lib.writer.transaction()?;
    for (p, bytes) in &s.files {
        match bytes {
            Some(b) => write_file(p, b)?,
            None => {
                if p.exists() {
                    std::fs::remove_file(p).map_err(|e| io(p, e))?;
                }
            }
        }
    }
    let c = &s.columns;
    for (name, v) in [
        ("AnalysisDataPath", &c.analysis_data_path),
        ("BPM", &c.bpm),
        ("SampleRate", &c.sample_rate),
        ("BitRate", &c.bit_rate),
        ("BitDepth", &c.bit_depth),
        ("Length", &c.length),
        ("Analysed", &c.analysed),
        ("ContentLink", &c.content_link),
    ] {
        tx.update("djmdContent", "ID", key(), &[(name, v.clone())])?;
    }
    // The previous key: its row, or a new row with its name when the analysis's orphan cleanup
    // removed it; then the analysis's key row goes if nothing uses it any more.
    if let Some((old_id, old_name)) = &s.key {
        let value = match (old_id, old_name) {
            (Value::Text(id), Some(name)) => {
                let live = tx
                    .connection()
                    .query_row("SELECT 1 FROM djmdKey WHERE ID = ?1 AND rb_local_deleted = 0", [id], |_| Ok(()))
                    .optional()?
                    .is_some();
                if live { old_id.clone() } else { Value::Text(find_or_create(&tx, NameTable::Key, name)?) }
            }
            _ => old_id.clone(),
        };
        tx.update("djmdContent", "ID", key(), &[("KeyID", value)])?;
        tx.cleanup_orphan_table(OrphanTable::Key)?;
        tx.increment_counter(s.content.as_str(), CounterColumn::TrackInfoUpdated)?;
    }
    if let Some(v) = &s.image_path {
        tx.update("djmdContent", "ID", key(), &[("ImagePath", v.clone())])?;
    }
    // Counters only move forward: the restored state is a new change for exports.
    tx.increment_counter(s.content.as_str(), CounterColumn::AnalysisUpdated)?;
    if let Some(id) = &s.mixer_inserted {
        tx.delete_rows("djmdMixerParam", "ID", &[Value::Text(id.clone())])?;
    }
    for (id, previous) in &s.content_files {
        match previous {
            None => {
                tx.delete_rows("contentFile", "ID", &[Value::Text(id.clone())])?;
            }
            Some((hash, size)) => {
                tx.update("contentFile", "ID", Value::Text(id.clone()), &[("Hash", Value::Text(hash.clone())), ("Size", Value::Integer(*size))])?;
            }
        }
    }
    tx.commit()?;
    sync_playlists(lib, &s.content)?;
    Ok((Effect::Done, None))
}
