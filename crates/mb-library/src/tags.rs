//! What import reads from an audio file: tags (via lofty), duration, file type, size and inode.
//! Spec: `docs/re/ops/import-track.md` § `djmdContent` insert. The value rules (title fallback,
//! numbers 0 when absent, strings verbatim) are verified against the 161 traced imports
//! (`tests/tags_calibration.rs`); which tag frame feeds which column is calibrated on that corpus.

use crate::{Error, Result};
use lofty::config::{ParseOptions, ParsingMode};
use lofty::file::{AudioFile, FileType as LoftyType, TaggedFile, TaggedFileExt};
use lofty::prelude::ItemKey;
use lofty::probe::Probe;
use lofty::tag::{Tag, TagType};
use std::path::Path;

/// `djmdContent.FileType` codes (import-track.md; verified on 161 files).
pub mod file_type {
    pub const MP3: i64 = 1;
    pub const M4A_AAC: i64 = 4;
    pub const FLAC: i64 = 5;
    pub const M4A_ALAC: i64 = 6;
    pub const WAV: i64 = 11;
    pub const AIFF: i64 = 12;
}

/// Everything import needs from one file. Empty strings mean "no tag".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileTags {
    /// Tag title, or the file name without extension when there is none.
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub remixer: String,
    pub label: String,
    /// Key tag, stored verbatim as `djmdKey.ScaleName`.
    pub key: String,
    pub comment: String,
    pub composer: String,
    pub original_artist: String,
    pub lyricist: String,
    pub track_no: i64,
    pub disc_no: i64,
    pub release_year: i64,
    /// Raw release-date tag (e.g. `2017-01-30`), `""` when absent.
    pub release_date: String,
    pub isrc: String,
    /// Whole seconds.
    pub length: i64,
    pub file_type: i64,
    pub file_size: i64,
    /// `rb_file_id`: the inode number (`juce::File::getFileIdentifier`).
    pub inode: u64,
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io { path: path.display().to_string(), source }
}

/// Reads `path`. Fails for files lofty cannot parse or rekordbox would not import.
pub fn read_file(path: &Path) -> Result<FileTags> {
    let meta = std::fs::metadata(path).map_err(io_err(path))?;
    let inode = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    let (lofty_type, duration, tag) = read_parts(path)?;
    let file_type = match lofty_type {
        LoftyType::Mpeg => file_type::MP3,
        LoftyType::Flac => file_type::FLAC,
        LoftyType::Wav => file_type::WAV,
        LoftyType::Aiff => file_type::AIFF,
        LoftyType::Mp4 => mp4_file_type(path)?,
        other => {
            return Err(Error::Tags { path: path.display().to_string(), message: format!("unsupported file type {other:?}") });
        }
    };
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut tags = FileTags {
        length: duration.as_secs() as i64,
        file_type,
        file_size: meta.len() as i64,
        inode,
        ..FileTags::default()
    };
    if let Some(tag) = &tag {
        fill_from_tag(&mut tags, tag);
        if tag.tag_type() == TagType::Id3v2
            && let Some(raw) = id3_bytes(path, lofty_type).and_then(|b| raw_id3_text(&b, b"TDRL"))
        {
            tags.release_date = complete_release_date(&raw);
        }
    }
    if tags.title.is_empty() {
        tags.title = stem;
    }
    Ok(tags)
}

fn options(mode: ParsingMode) -> ParseOptions {
    ParseOptions::new().read_cover_art(false).parsing_mode(mode)
}

fn tag_error(path: &Path, e: &dyn std::error::Error) -> Error {
    Error::Tags { path: path.display().to_string(), message: chain(e) }
}

fn probe_path(path: &Path, opts: ParseOptions) -> Result<TaggedFile> {
    Probe::open(path).map_err(|e| tag_error(path, &e))?.options(opts).read().map_err(|e| tag_error(path, &e))
}

/// (container, duration, the tag import reads). Real-world files trip lofty in different places:
/// Relaxed tolerates a non-UTF-8 AIFF `NAME` chunk but not an ID3 `UFID` frame with an empty
/// owner; BestAttempt the other way round. If both fail, the audio properties are read without
/// tags and, for AIFF/WAV, the `ID3 ` chunk alone is parsed (rekordbox reads its tags from there;
/// verified on `Cosmic Boys - Titan`, whose `NAME` chunk is not UTF-8).
fn read_parts(path: &Path) -> Result<(LoftyType, std::time::Duration, Option<Tag>)> {
    let mut first_error = None;
    for mode in [ParsingMode::Relaxed, ParsingMode::BestAttempt] {
        match probe_path(path, options(mode)) {
            Ok(f) => return Ok((f.file_type(), f.properties().duration(), pick_tag(&f).cloned())),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    let bare = probe_path(path, options(ParsingMode::Relaxed).read_tags(false)).map_err(|_| first_error.unwrap())?;
    let tag = match bare.file_type() {
        LoftyType::Aiff | LoftyType::Wav => iff_id3_only(path, bare.file_type()).ok().flatten(),
        _ => None,
    };
    Ok((bare.file_type(), bare.properties().duration(), tag))
}

/// (chunk ID, body) of an IFF chunk.
type Chunk = ([u8; 4], Vec<u8>);

/// The 12-byte container header and the chunks `wanted` selects, of an AIFF (big-endian sizes)
/// or WAV (little-endian) file.
fn iff_chunks(path: &Path, big_endian: bool, wanted: impl Fn(&[u8; 4]) -> bool) -> Result<([u8; 12], Vec<Chunk>)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(io_err(path))?;
    let len = f.metadata().map_err(io_err(path))?.len();
    let mut header = [0u8; 12];
    f.read_exact(&mut header).map_err(io_err(path))?;
    let mut keep = Vec::new();
    let mut pos = 12u64;
    while pos + 8 <= len {
        let mut ch = [0u8; 8];
        f.seek(SeekFrom::Start(pos)).map_err(io_err(path))?;
        f.read_exact(&mut ch).map_err(io_err(path))?;
        let id: [u8; 4] = ch[..4].try_into().unwrap();
        let size_bytes: [u8; 4] = ch[4..].try_into().unwrap();
        let size = if big_endian { u32::from_be_bytes(size_bytes) } else { u32::from_le_bytes(size_bytes) } as u64;
        if wanted(&id) {
            let mut body = vec![0u8; size.min(len - pos - 8) as usize];
            f.read_exact(&mut body).map_err(io_err(path))?;
            keep.push((id, body));
        }
        pos += 8 + size + (size & 1);
    }
    Ok((header, keep))
}

fn is_id3_chunk(id: &[u8; 4]) -> bool {
    id.eq_ignore_ascii_case(b"ID3 ")
}

/// Rebuilds an AIFF/WAV in memory from the format chunk and the `ID3 ` chunk only, and reads its
/// tag. Used when another chunk of the real file makes lofty fail.
fn iff_id3_only(path: &Path, kind: LoftyType) -> Result<Option<Tag>> {
    let big_endian = kind == LoftyType::Aiff;
    let format_id: &[u8; 4] = if big_endian { b"COMM" } else { b"fmt " };
    let (header, keep) = iff_chunks(path, big_endian, |id| id == format_id || is_id3_chunk(id))?;
    if !keep.iter().any(|(id, _)| is_id3_chunk(id)) {
        return Ok(None);
    }
    let size = |n: usize| if big_endian { (n as u32).to_be_bytes() } else { (n as u32).to_le_bytes() };
    let mut body = header[8..12].to_vec();
    for (id, data) in &keep {
        body.extend_from_slice(id);
        body.extend_from_slice(&size(data.len()));
        body.extend_from_slice(data);
        if data.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut file = header[..4].to_vec();
    file.extend_from_slice(&size(body.len()));
    file.extend_from_slice(&body);
    let mut first_error = None;
    for mode in [ParsingMode::Relaxed, ParsingMode::BestAttempt] {
        let parsed = Probe::new(std::io::Cursor::new(file.as_slice()))
            .options(options(mode).read_properties(false))
            .guess_file_type()
            .map_err(|e| tag_error(path, &e))?
            .read();
        match parsed {
            Ok(tagged) => return Ok(pick_tag(&tagged).cloned()),
            Err(e) => {
                first_error.get_or_insert(tag_error(path, &e));
            }
        }
    }
    Err(first_error.unwrap())
}

/// The raw ID3v2 tag of an MP3 (at the start) or AIFF/WAV (`ID3 ` chunk).
fn id3_bytes(path: &Path, kind: LoftyType) -> Option<Vec<u8>> {
    use std::io::Read;
    match kind {
        LoftyType::Mpeg => {
            let mut f = std::fs::File::open(path).ok()?;
            let mut head = [0u8; 10];
            f.read_exact(&mut head).ok()?;
            if &head[..3] != b"ID3" {
                return None;
            }
            let size = syncsafe(&head[6..10]) as usize;
            let mut rest = vec![0u8; size];
            f.read_exact(&mut rest).ok()?;
            let mut all = head.to_vec();
            all.extend_from_slice(&rest);
            Some(all)
        }
        LoftyType::Aiff | LoftyType::Wav => {
            let (_, chunks) = iff_chunks(path, kind == LoftyType::Aiff, is_id3_chunk).ok()?;
            chunks.into_iter().next().map(|(_, body)| body)
        }
        _ => None,
    }
}

fn syncsafe(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |acc, x| (acc << 7) | u32::from(x & 0x7F))
}

/// The text of the first `frame_id` frame of an ID3v2.3/2.4 tag, decoded but not interpreted
/// (lofty parses `TDRL` as a timestamp and drops values like `02.20.2017-01-01`, which rekordbox
/// stores verbatim). `None` for v2.2, unsynchronised tags or a missing frame.
fn raw_id3_text(tag: &[u8], frame_id: &[u8; 4]) -> Option<String> {
    if tag.len() < 10 || &tag[..3] != b"ID3" || !(3..=4).contains(&tag[3]) || tag[5] & 0x80 != 0 {
        return None;
    }
    let v4 = tag[3] == 4;
    let end = (10 + syncsafe(&tag[6..10]) as usize).min(tag.len());
    let mut pos = 10;
    if tag[5] & 0x40 != 0 {
        let b = tag.get(10..14)?;
        pos += if v4 { syncsafe(b) as usize } else { u32::from_be_bytes(b.try_into().ok()?) as usize + 4 };
    }
    while pos + 10 <= end {
        let id = &tag[pos..pos + 4];
        if id[0] == 0 {
            break;
        }
        let size = if v4 { syncsafe(&tag[pos + 4..pos + 8]) } else { u32::from_be_bytes(tag[pos + 4..pos + 8].try_into().ok()?) } as usize;
        let body = tag.get(pos + 10..(pos + 10 + size).min(end))?;
        if id == frame_id {
            let (&enc, text) = body.split_first()?;
            let s = match enc {
                0 => text.iter().map(|&b| b as char).collect::<String>(),
                3 => String::from_utf8_lossy(text).into_owned(),
                1 | 2 => {
                    let mut be = enc == 2;
                    let mut t = text;
                    if t.len() >= 2 && (t[..2] == [0xFF, 0xFE] || t[..2] == [0xFE, 0xFF]) {
                        be = t[0] == 0xFE;
                        t = &t[2..];
                    }
                    let units: Vec<u16> = t.chunks_exact(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) }).collect();
                    String::from_utf16_lossy(&units)
                }
                _ => return None,
            };
            return Some(s.split('\0').next().unwrap_or("").to_owned());
        }
        pos += 10 + size;
    }
    None
}

/// MP4: AAC → 4, ALAC → 6 (import-track.md, by codec).
fn mp4_file_type(path: &Path) -> Result<i64> {
    use lofty::mp4::{Mp4Codec, Mp4File};
    let mut file = std::fs::File::open(path).map_err(io_err(path))?;
    let mp4 = Mp4File::read_from(&mut file, options(ParsingMode::Relaxed).read_tags(false))
        .map_err(|e| tag_error(path, &e))?;
    Ok(match mp4.properties().codec() {
        Some(Mp4Codec::ALAC) => file_type::M4A_ALAC,
        _ => file_type::M4A_AAC,
    })
}

fn pick_tag(file: &TaggedFile) -> Option<&Tag> {
    file.primary_tag().or_else(|| file.first_tag())
}

fn text(tag: &Tag, key: ItemKey) -> String {
    tag.get_string(key).map(str::to_owned).unwrap_or_default()
}

/// Leading decimal digits of a number tag (`3/12` → 3); 0 when absent.
fn leading_number(s: &str) -> i64 {
    let digits: String = s.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

fn fill_from_tag(t: &mut FileTags, tag: &Tag) {
    t.title = text(tag, ItemKey::TrackTitle);
    t.artist = text(tag, ItemKey::TrackArtist);
    t.album = text(tag, ItemKey::AlbumTitle);
    // Vorbis comments (FLAC): rekordbox ignored `ALBUMARTIST` and `publisher` on the corpus
    // (`Audiomatiques,Cosmic Boys - Let's Go`, 15 files with `publisher`), so neither is read.
    let vorbis = tag.tag_type() == TagType::VorbisComments;
    t.album_artist = if vorbis { String::new() } else { text(tag, ItemKey::AlbumArtist) };
    t.genre = text(tag, ItemKey::Genre);
    t.remixer = text(tag, ItemKey::Remixer);
    t.label = if vorbis {
        // hypothesis: a Vorbis `LABEL` comment is read (no corpus file has one).
        text(tag, ItemKey::Label)
    } else {
        tag.get_string(ItemKey::Label).or_else(|| tag.get_string(ItemKey::Publisher)).map(str::to_owned).unwrap_or_default()
    };
    t.key = text(tag, ItemKey::InitialKey);
    t.comment = text(tag, ItemKey::Comment);
    t.composer = text(tag, ItemKey::Composer);
    t.original_artist = text(tag, ItemKey::OriginalArtist);
    t.lyricist = text(tag, ItemKey::Lyricist);
    t.track_no = leading_number(&text(tag, ItemKey::TrackNumber));
    t.disc_no = leading_number(&text(tag, ItemKey::DiscNumber));
    let date = tag.get_string(ItemKey::RecordingDate).or_else(|| tag.get_string(ItemKey::Year)).unwrap_or("");
    t.release_year = leading_number(date);
    t.release_date = text(tag, ItemKey::ReleaseDate);
    t.isrc = text(tag, ItemKey::Isrc);
}

/// rekordbox completes a release date that is not a full `YYYY-MM-DD`: the corpus file with
/// `TDRL = 02.20.2017` was stored as `02.20.2017-01-01`; the 29 full dates were stored verbatim.
// hypothesis: the completion counts dashes, so `2017-05` would become `2017-05-01` (unobserved).
fn complete_release_date(raw: &str) -> String {
    match raw.matches('-').count() {
        _ if raw.is_empty() => String::new(),
        0 => format!("{raw}-01-01"),
        1 => format!("{raw}-01"),
        _ => raw.to_owned(),
    }
}

/// The items of the tag import reads, for calibration and debugging.
pub fn dump(path: &Path) -> Result<Vec<String>> {
    let (kind, duration, tag) = read_parts(path)?;
    let mut out = vec![format!("type {kind:?} duration {duration:?} tag {:?}", tag.as_ref().map(|t| t.tag_type()))];
    if tag.is_none() && matches!(kind, LoftyType::Aiff | LoftyType::Wav) {
        out.push(format!("ID3-chunk fallback: {:?}", iff_id3_only(path, kind).map(|t| t.map(|t| t.item_count()))));
    }
    for item in tag.iter().flat_map(|t| t.items()) {
        out.push(format!("{:?} = {:?}", item.key(), item.value()));
    }
    Ok(out)
}

/// The error and its sources, joined.
fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        out.push_str(": ");
        out.push_str(&s.to_string());
        cur = s.source();
    }
    out
}
