//! How rekordbox 7.2.19 turns its library into rekordbox.xml values (`db::DjPlaylistsWriter`,
//! docs/re/xml.md § Derivation). Everything here is verified against fixtures/oracle-a unless the
//! doc comment says otherwise.

use crate::model::{
    Document, Extra, KeyType, PlaylistEntry, PlaylistNode, PositionMark, Product, Tempo, Track,
};
use mb_core::{FileType, LibrarySnapshot, PlaylistKind};

/// `DJ_PLAYLISTS Version` (global `xmlVersion`).
pub const XML_VERSION: &str = "1.0.0";

impl Document {
    /// An empty document with rekordbox's root, product node, collection and `ROOT` folder.
    pub fn rekordbox(app_version: &str) -> Self {
        Self {
            version: Some(XML_VERSION.into()),
            product: Some(Product::rekordbox(app_version)),
            collection: Some(Vec::new()),
            playlists: Some(vec![PlaylistNode::Folder {
                name: "ROOT".into(),
                children: Vec::new(),
                extra: Extra::default(),
            }]),
            ..Self::default()
        }
    }
}

/// `TrackID` and playlist `Key`: rekordbox formats the u32 row ID with `juce::String(int)`, so IDs
/// of 2^31 and above come out negative.
pub fn xml_id(id: u32) -> i64 {
    i64::from(id as i32)
}

/// Inverse of [`xml_id`].
pub fn db_id(xml: i64) -> u32 {
    xml as i32 as u32
}

/// Seconds as rekordbox computes them for `Inizio`/`Start`/`End`: `(double)ms * 0.001`.
pub fn seconds_from_ms(ms: u32) -> f64 {
    f64::from(ms) * 0.001
}

/// BPM as rekordbox computes it for `AverageBpm`/`Bpm`: `(double)bpm_x100 * 0.01`.
pub fn bpm_from_x100(bpm_x100: u32) -> f64 {
    f64::from(bpm_x100) * 0.01
}

/// `Rating` = stars × 51 (`addTrackNode`: `(char)rating * 0x33`), so 0, 51, 102, 153, 204, 255.
pub fn rating_to_xml(stars: u8) -> u8 {
    stars.min(5) * 51
}

/// Import direction (`DjPlaylistsAnalyzer::analyzeTrack`): integer division by 51.
pub fn rating_from_xml(value: i64) -> u8 {
    (value / 51).clamp(0, 255) as u8
}

/// The eight track colours: (`djmdContent.ColorID`, default name = `djmdColor.Commnt`, `Colour`).
/// `addTrackNode` writes `Colour` only for IDs 1..=8; the import compares the same strings
/// case-insensitively. The RGB values are constants in the binary (no fixture track has a colour).
pub const TRACK_COLOURS: [(u8, &str, u32); 8] = [
    (1, "Pink", 0xFF007F),
    (2, "Red", 0xFF0000),
    (3, "Orange", 0xFFA500),
    (4, "Yellow", 0xFFFF00),
    (5, "Green", 0x00FF00),
    (6, "Aqua", 0x25FDE9),
    (7, "Blue", 0x0000FF),
    (8, "Purple", 0x660099),
];

pub fn colour_rgb(color_id: u8) -> Option<u32> {
    TRACK_COLOURS.iter().find(|c| c.0 == color_id).map(|c| c.2)
}

pub fn colour_id(rgb: u32) -> Option<u8> {
    TRACK_COLOURS.iter().find(|c| c.2 == rgb).map(|c| c.0)
}

/// Default colour name, which rekordbox writes as `Grouping` (hypothesis: the writer reads the
/// same RowDataTrack string the import fills with the colour name; `djmdColor.Commnt`).
pub fn colour_name(color_id: u8) -> Option<&'static str> {
    TRACK_COLOURS.iter().find(|c| c.0 == color_id).map(|c| c.1)
}

/// UI language of the `Kind` labels. `addTrackNode` passes English keys through `juce::translate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindLanguage {
    English,
    German,
}

/// (`djmdContent.FileType`, English key, German translation from `locale/german.lang`).
/// Note the trailing spaces in four German labels; the fixture has them too.
const KINDS: [(i64, &str, &str); 8] = [
    (1, "MP3 File", "Mp3-Datei "),
    (2, "AAC File", "Aac-Datei "),
    (3, "MP4 File", "Mp4-Datei "),
    (4, "M4A File", "M4a-Datei "),
    (5, "FLAC File", "FLAC-Datei"),
    (6, "ALAC File", "ALAC-Datei"),
    (11, "WAV File", "Wav-Datei "),
    (12, "AIFF File", "Aiff-Datei "),
];
const UNKNOWN_KIND: (&str, &str) = ("Unknown Format", "Unbekanntes Format");

/// `Kind` for a `djmdContent.FileType` code.
pub fn kind_label(file_type: i64, language: KindLanguage) -> &'static str {
    let pick = |en: &'static str, de: &'static str| match language {
        KindLanguage::English => en,
        KindLanguage::German => de,
    };
    KINDS
        .iter()
        .find(|k| k.0 == file_type)
        .map_or_else(|| pick(UNKNOWN_KIND.0, UNKNOWN_KIND.1), |k| pick(k.1, k.2))
}

/// The FileType code and language a `Kind` label was written with.
pub fn kind_from_label(label: &str) -> Option<(i64, KindLanguage)> {
    KINDS.iter().find_map(|k| {
        if k.1 == label {
            Some((k.0, KindLanguage::English))
        } else if k.2 == label {
            Some((k.0, KindLanguage::German))
        } else {
            None
        }
    })
}

pub fn file_type_code(t: FileType) -> i64 {
    match t {
        FileType::Mp3 => 1,
        FileType::M4a => 4,
        FileType::Flac => 5,
        FileType::Alac => 6,
        FileType::Wav => 11,
        FileType::Aiff => 12,
        FileType::Other(c) => c,
    }
}

/// Bytes `locationToURI` @0x101056ce0 leaves alone besides ASCII letters and digits.
const URI_SAFE: &[u8] = b"!#$()*+,-./:;=?@_~";

/// `Location`: `file://localhost` + the path's UTF-8 bytes, where every byte that is not an ASCII
/// letter, digit or one of `!#$()*+,-./:;=?@_~` becomes `%xx` with lowercase hex.
/// Verified on all 161 fixture tracks (spaces, `&`, `'`, non-ASCII bytes).
pub fn location_to_uri(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 32);
    out.push_str("file://localhost");
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || URI_SAFE.contains(&b) {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(b"0123456789abcdef"[usize::from(b >> 4)]));
            out.push(char::from(b"0123456789abcdef"[usize::from(b & 15)]));
        }
    }
    out
}

/// Import direction (`DjPlaylistsAnalyzer::locationFromURI` @0x101053ed0): decode every `%xx`
/// (either case), then keep what follows the first `file://localhost` (case-insensitive).
/// `None` when the prefix is missing (rekordbox gets an empty path then).
pub fn location_from_uri(uri: &str) -> Option<String> {
    let bytes = uri.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|b| hex(*b)),
                bytes.get(i + 2).and_then(|b| hex(*b)),
            )
        {
            decoded.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    let text = String::from_utf8_lossy(&decoded);
    let lower = text.to_ascii_lowercase();
    let at = lower.find("file://localhost")?;
    Some(text[at + "file://localhost".len()..].to_owned())
}

/// One PQTZ beat as `addTempoNode` reads it from `TrackInfo` (+0x60, 8 bytes each).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridBeat {
    pub beat_in_bar: u16,
    pub tempo_x100: u16,
    pub time_ms: u32,
}

/// rekordbox setting `bridgeExportBeatGrid` (default 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempoMode {
    /// 0 (or negative, or 3 and above): no TEMPO elements.
    Off,
    /// 1: one TEMPO per tempo change or grid discontinuity (see [`tempo_anchors`]).
    Changes,
    /// 2: one TEMPO per beat.
    EveryBeat,
}

/// Why a beat starts a TEMPO entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorReason {
    /// Its tempo differs from the previous beat's (always true for the first beat).
    TempoChange,
    /// Same tempo, but its time is off the predicted grid.
    Discontinuity,
}

/// The beats `addTempoNode` @0x101057074 turns into TEMPO entries in mode 1.
///
/// rekordbox keeps two predictions of the next beat time, both advanced by the beat length
/// `6000000.0 / tempo_x100` ms after every beat: `grid`, accumulated from the last anchor, and
/// `local`, from the previous beat's own time. A beat is an anchor when its tempo differs from the
/// previous one, or when its time matches none of `trunc(grid)`, `trunc(grid + 0.5)`,
/// `trunc(local)`, `trunc(local + 0.5)`. At an anchor `grid` restarts at the beat's time unless
/// the time equals `trunc(grid)` or `trunc(grid + 0.5)`, in which case the fractional prediction is
/// kept. Verified: reproduces all 242 TEMPO entries of the 161 fixture tracks.
pub fn tempo_anchors(beats: &[GridBeat]) -> Vec<(usize, AnchorReason)> {
    // fcvtzu: truncation toward zero, saturating; `as u64` does the same.
    let trunc = |v: f64| v as u64;
    let (mut grid, mut local, mut last_tempo) = (0.0f64, 0.0f64, 0u16);
    let mut out = Vec::new();
    for (i, b) in beats.iter().enumerate() {
        let t = u64::from(b.time_ms);
        let tempo_changed = b.tempo_x100 != last_tempo;
        let on_grid = t == trunc(grid)
            || t == trunc(grid + 0.5)
            || t == trunc(local)
            || t == trunc(local + 0.5);
        if tempo_changed || !on_grid {
            out.push((
                i,
                if tempo_changed {
                    AnchorReason::TempoChange
                } else {
                    AnchorReason::Discontinuity
                },
            ));
            let restart = if t == trunc(grid + 0.5) {
                grid
            } else {
                t as f64
            };
            last_tempo = b.tempo_x100;
            if t != trunc(grid) {
                grid = restart;
            }
        }
        let beat_ms = 6_000_000.0 / f64::from(b.tempo_x100);
        grid += beat_ms;
        local = beat_ms + t as f64;
    }
    out
}

/// TEMPO entry for one beat: `Inizio` = ms × 0.001, `Bpm` = tempo × 0.01, `Metro` = `4/4`,
/// `Battito` = beat in bar.
pub fn tempo_for_beat(b: &GridBeat) -> Tempo {
    Tempo {
        inizio: seconds_from_ms(b.time_ms),
        bpm: bpm_from_x100(u32::from(b.tempo_x100)),
        metro: "4/4".into(),
        battito: u32::from(b.beat_in_bar),
        extra: Extra::default(),
    }
}

/// The TEMPO elements rekordbox writes for a PQTZ beat grid.
pub fn tempos_from_beat_grid(beats: &[GridBeat], mode: TempoMode) -> Vec<Tempo> {
    match mode {
        TempoMode::Off => Vec::new(),
        TempoMode::EveryBeat => beats.iter().map(tempo_for_beat).collect(),
        TempoMode::Changes => tempo_anchors(beats)
            .into_iter()
            .map(|(i, _)| tempo_for_beat(&beats[i]))
            .collect(),
    }
}

/// `Num` for a cue kind (`analyzer::Cue` +0x0c, the `djmdCue.Kind` value): 1..=3 → 0..=2 (A–C),
/// 5..=9 → 3..=7 (D–H), -1, 0 and 4 → -1 (memory cue); other kinds are not exported.
/// From `addPositionMarkNode` @0x1010574b0; no fixture has cues.
pub fn position_mark_num(kind: i64) -> Option<i32> {
    match kind {
        1..=3 => Some(kind as i32 - 1),
        5..=9 => Some(kind as i32 - 2),
        -1 | 0 | 4 => Some(-1),
        _ => None,
    }
}

/// The POSITION_MARK rekordbox writes for a cue: `Type` 4 with `End` when the end position is
/// non-zero, else `Type` 0; positions are ms × 0.001; `rgb` is written for hot cues only
/// (`getHotCueDefaultColor` @0x10162e784 resolves the colour; pass its result).
pub fn position_mark(
    kind: i64,
    name: &str,
    start_ms: u32,
    end_ms: u32,
    rgb: Option<[u8; 3]>,
) -> Option<PositionMark> {
    let num = position_mark_num(kind)?;
    Some(PositionMark {
        name: Some(name.to_owned()),
        mark_type: if end_ms != 0 { 4 } else { 0 },
        start: seconds_from_ms(start_ms),
        end: (end_ms != 0).then(|| seconds_from_ms(end_ms)),
        num,
        rgb: if num >= 0 { rgb } else { None },
        extra: Extra::default(),
    })
}

/// The TRACK element rekordbox writes for a collection entry (`addTrackNode` @0x101055910).
///
/// `mb_core::Track` has no `Subtitle` (`Mix`) and no custom colour names, so `mix` is empty and
/// `grouping` uses the default colour name; set them afterwards if known. Cues are not part of the
/// snapshot either: add `position_marks` with [`position_mark`]. Integer widths follow the writer
/// (`TotalTime`, `DiscNumber`, `Year`, `PlayCount` are u16; `TrackID`, `Size` are i32).
pub fn track_from_core(t: &mb_core::Track, language: KindLanguage, beats: &[GridBeat]) -> Track {
    let u16_of = |v: Option<u32>| u32::from(v.unwrap_or(0) as u16);
    Track {
        track_id: t.id.as_str().parse::<u32>().ok().map(xml_id),
        name: Some(t.title.clone()),
        artist: Some(t.artist.clone()),
        composer: Some(t.composer.clone()),
        album: Some(t.album.clone()),
        grouping: Some(t.color.and_then(colour_name).unwrap_or("").to_owned()),
        genre: Some(t.genre.clone()),
        kind: Some(kind_label(file_type_code(t.file_type), language).to_owned()),
        size: Some(i64::from(t.file_size.unwrap_or(0) as i32)),
        total_time: Some(u16_of(t.length_seconds)),
        disc_number: Some(u16_of(t.disc_no)),
        track_number: Some(t.track_no.unwrap_or(0)),
        year: Some(u16_of(t.release_year)),
        average_bpm: Some(bpm_from_x100(t.bpm_x100.unwrap_or(0))),
        date_added: Some(t.date_added.clone()),
        bit_rate: Some(t.bitrate.unwrap_or(0)),
        sample_rate: Some(t.sample_rate.unwrap_or(0)),
        comments: Some(t.comment.clone()),
        play_count: Some(u32::from(t.play_count as u16)),
        rating: Some(rating_to_xml(t.rating)),
        location: Some(location_to_uri(&t.path.to_string_lossy())),
        remixer: Some(t.remixer.clone()),
        tonality: Some(t.key.clone()),
        label: Some(t.label.clone()),
        mix: Some(String::new()),
        colour: t.color.and_then(colour_rgb),
        tempos: tempos_from_beat_grid(beats, TempoMode::Changes),
        position_marks: Vec::new(),
        extra: Extra::default(),
    }
}

/// The `ROOT` folder rekordbox writes for a playlist tree: siblings in `Seq` order, folders with
/// their children, playlists with `KeyType="0"` and one `TRACK Key` per entry in playlist order.
/// Smart playlists are written as plain playlists of their current result (rekordbox evaluates
/// them; the snapshot has no entries for them, so they come out empty here).
pub fn playlists_from_snapshot(snapshot: &LibrarySnapshot) -> PlaylistNode {
    fn children(s: &LibrarySnapshot, parent: Option<&mb_core::DbId>) -> Vec<PlaylistNode> {
        let mut nodes: Vec<&mb_core::PlaylistNode> = s
            .playlists
            .iter()
            .filter(|n| n.parent.as_ref() == parent)
            .collect();
        nodes.sort_by_key(|n| n.seq);
        nodes
            .into_iter()
            .map(|n| match n.kind {
                PlaylistKind::Folder => PlaylistNode::Folder {
                    name: n.name.clone(),
                    children: children(s, Some(&n.id)),
                    extra: Extra::default(),
                },
                PlaylistKind::Playlist | PlaylistKind::Smart => PlaylistNode::Playlist {
                    name: n.name.clone(),
                    key_type: Some(KeyType::TrackId),
                    entries: s
                        .playlist_entries
                        .get(&n.id)
                        .map(|ids| {
                            ids.iter()
                                .map(|id| match id.as_str().parse::<u32>() {
                                    Ok(v) => PlaylistEntry::track_id(xml_id(v)),
                                    Err(_) => PlaylistEntry {
                                        key: id.0.clone(),
                                        extra: Extra::default(),
                                    },
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    extra: Extra::default(),
                },
            })
            .collect()
    }
    PlaylistNode::Folder {
        name: "ROOT".into(),
        children: children(snapshot, None),
        extra: Extra::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_encoding() {
        let path = "/Users/x/a b&c'd(é)[1].mp3";
        let uri = location_to_uri(path);
        assert_eq!(
            uri,
            "file://localhost/Users/x/a%20b%26c%27d(%c3%a9)%5b1%5d.mp3"
        );
        assert_eq!(location_from_uri(&uri).as_deref(), Some(path));
        assert_eq!(
            location_from_uri("FILE://LOCALHOST/a%2Fb").as_deref(),
            Some("/a/b")
        );
        assert_eq!(location_from_uri("/no/prefix"), None);
    }

    #[test]
    fn ids_are_signed_32_bit() {
        assert_eq!(xml_id(89904879), 89904879);
        assert_eq!(xml_id(2736485380), -1558481916);
        assert_eq!(db_id(-1558481916), 2736485380);
    }

    #[test]
    fn kinds_and_ratings() {
        assert_eq!(kind_label(1, KindLanguage::German), "Mp3-Datei ");
        assert_eq!(kind_label(6, KindLanguage::English), "ALAC File");
        assert_eq!(kind_label(99, KindLanguage::English), "Unknown Format");
        assert_eq!(
            kind_from_label("Aiff-Datei "),
            Some((12, KindLanguage::German))
        );
        assert_eq!(
            (0..=5).map(rating_to_xml).collect::<Vec<_>>(),
            [0, 51, 102, 153, 204, 255]
        );
        assert_eq!(rating_from_xml(204), 4);
        assert_eq!(rating_from_xml(50), 0);
    }

    #[test]
    fn cue_kinds() {
        let nums: Vec<_> = (-2..=10).map(position_mark_num).collect();
        assert_eq!(
            nums,
            [
                None,
                Some(-1),
                Some(-1),
                Some(0),
                Some(1),
                Some(2),
                Some(-1),
                Some(3),
                Some(4),
                Some(5),
                Some(6),
                Some(7),
                None
            ]
        );
        let m = position_mark(2, "x", 1234, 5678, Some([1, 2, 3])).unwrap();
        assert_eq!(
            (m.mark_type, m.num, m.end, m.rgb),
            (4, 1, Some(5.678), Some([1, 2, 3]))
        );
        let m = position_mark(0, "", 1234, 0, Some([1, 2, 3])).unwrap();
        assert_eq!((m.mark_type, m.num, m.end, m.rgb), (0, -1, None, None));
    }

    fn grid(start_ms: u32, tempo_x100: u16, n: usize, first_beat: u16) -> Vec<GridBeat> {
        (0..n)
            .map(|i| GridBeat {
                beat_in_bar: ((usize::from(first_beat) - 1 + i) % 4 + 1) as u16,
                tempo_x100,
                time_ms: start_ms + (i as f64 * 6_000_000.0 / f64::from(tempo_x100)).round() as u32,
            })
            .collect()
    }

    #[test]
    fn constant_grid_is_one_tempo() {
        let beats = grid(1231, 12800, 300, 3);
        let tempos = tempos_from_beat_grid(&beats, TempoMode::Changes);
        assert_eq!(tempos.len(), 1);
        assert_eq!(
            (tempos[0].inizio, tempos[0].bpm, tempos[0].battito),
            (1.231, 128.0, 3)
        );
    }

    #[test]
    fn tempo_change_and_jump_start_new_entries() {
        let mut beats = grid(19, 12000, 8, 1);
        let after = grid(beats.last().unwrap().time_ms + 495, 12100, 8, 1);
        beats.extend(after);
        let jump = grid(beats.last().unwrap().time_ms + 1000, 12100, 4, 1);
        beats.extend(jump);
        let anchors = tempo_anchors(&beats);
        assert_eq!(
            anchors,
            [
                (0, AnchorReason::TempoChange),
                (8, AnchorReason::TempoChange),
                (16, AnchorReason::Discontinuity)
            ]
        );
        assert_eq!(
            tempos_from_beat_grid(&beats, TempoMode::EveryBeat).len(),
            beats.len()
        );
        assert!(tempos_from_beat_grid(&beats, TempoMode::Off).is_empty());
    }
}
