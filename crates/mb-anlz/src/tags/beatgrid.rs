//! PQTZ (beat grid, .DAT) and PQT2 (extended beat grid, .EXT).
//! Writers `write_pmai_quantize` @0x100f5b0c8 / `write_pmai_quantize_data` @0x100f5b704 and
//! `write_pmai_ext_quantize_ext` @0x1013d09a0; PQT2 setter `set_pmai_ext_ext_beat_position` @0x100f2667c.
//! Spec: `docs/re/anlz.md` § PQTZ, § PQT2.

use super::{Put, Rd, TagResult, check_tag};
use crate::{Section, Tag};

/// One beat: 8 bytes, big-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Beat {
    /// Position of the beat in its bar, 1..=4.
    pub beat_in_bar: u16,
    /// Tempo in BPM × 100 (12800 = 128.00 BPM).
    pub tempo: u16,
    /// Time of the beat in milliseconds from the start of the track.
    pub time_ms: u32,
}

impl Beat {
    pub fn bpm(&self) -> f64 {
        f64::from(self.tempo) / 100.0
    }

    fn read(r: &mut Rd) -> TagResult<Self> {
        Ok(Self {
            beat_in_bar: r.u16()?,
            tempo: r.u16()?,
            time_ms: r.u32()?,
        })
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.put_u16(self.beat_in_bar);
        out.put_u16(self.tempo);
        out.put_u32(self.time_ms);
    }
}

/// PQTZ section: the beat grid.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | `unk_0e` (0) |
/// | 0x10 | u16 | `unk_10` (8, set by `initialize_pmai`; hypothesis: entry size) |
/// | 0x12 | i16 | `adjust_offset` (beat adjust offset from `set_pmai_beat_position`; 0 in fixtures) |
/// | 0x14 | u32 | beat count |
/// | 0x18 | [`Beat`] × count | beats |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeatGrid {
    pub unk_0c: u16,
    pub unk_0e: u16,
    pub unk_10: u16,
    pub adjust_offset: i16,
    pub beats: Vec<Beat>,
}

impl BeatGrid {
    pub fn new(beats: Vec<Beat>) -> Self {
        Self {
            unk_0c: 0,
            unk_0e: 0,
            unk_10: 8,
            adjust_offset: 0,
            beats,
        }
    }

    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PQTZ])?;
        let mut h = Rd::new(s.tag, &s.header);
        let (unk_0c, unk_0e, unk_10, adjust_offset) =
            (h.u16()?, h.u16()?, h.u16()?, h.u16()? as i16);
        let count = h.u32()? as usize;
        h.finish()?;
        if count.checked_mul(8) != Some(s.body.len()) {
            return h.err("beat count does not match body");
        }
        let mut b = Rd::new(s.tag, &s.body);
        let beats = (0..count)
            .map(|_| Beat::read(&mut b))
            .collect::<TagResult<_>>()?;
        Ok(Self {
            unk_0c,
            unk_0e,
            unk_10,
            adjust_offset,
            beats,
        })
    }

    pub fn encode(&self) -> Section {
        let mut header = Vec::with_capacity(12);
        header.put_u16(self.unk_0c);
        header.put_u16(self.unk_0e);
        header.put_u16(self.unk_10);
        header.put_u16(self.adjust_offset as u16);
        header.put_u32(self.beats.len() as u32);
        let mut body = Vec::with_capacity(self.beats.len() * 8);
        for beat in &self.beats {
            beat.write(&mut body);
        }
        Section {
            tag: Tag::PQTZ,
            header,
            body,
        }
    }
}

/// PQT2 section: extended beat grid. The beats themselves are not repeated; the header carries the
/// first and last beat plus a checksum over the full grid, and the body has one u16 per beat.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0x0c | u16 | `unk_0c` (0) |
/// | 0x0e | u16 | `unk_0e` (0) |
/// | 0x10 | u8 | `analysis_type` (1 in fixtures; read by `get_pmai_ext_beat_analysis_type`) |
/// | 0x11 | u8 | `unk_11` (0) |
/// | 0x12 | u16 | entry size (2) |
/// | 0x14 | u32 | `unk_14` (setter stores 0) |
/// | 0x18 | [`Beat`] | first beat |
/// | 0x20 | [`Beat`] | last beat |
/// | 0x28 | u32 | entry count (= PQTZ beat count) |
/// | 0x2c | u32 | `checksum`: Σ(beat_in_bar + tempo + time_ms) over all beats, wrapping |
/// | 0x30 | u32 | `beat_num_offset` (present when the header is 0x38 long) |
/// | 0x34 | u32 | `unk_34` (present when the header is 0x38 long) |
/// | 0x38 | u16 × count | per-beat value; hypothesis: sub-millisecond part of the beat time in µs (0..=999) |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtBeatGrid {
    pub unk_0c: u16,
    pub unk_0e: u16,
    pub analysis_type: u8,
    pub unk_11: u8,
    pub unk_14: u32,
    pub first: Beat,
    pub last: Beat,
    pub checksum: u32,
    /// `Some((beat_num_offset, unk_34))` when the header is 0x38 bytes (every 7.x file).
    pub tail: Option<(u32, u32)>,
    pub entries: Vec<u16>,
}

impl ExtBeatGrid {
    /// The checksum rekordbox stores at 0x2c, over a full beat grid (verified 681/681 fixtures).
    pub fn checksum_of(beats: &[Beat]) -> u32 {
        beats.iter().fold(0u32, |acc, b| {
            acc.wrapping_add(u32::from(b.beat_in_bar))
                .wrapping_add(u32::from(b.tempo))
                .wrapping_add(b.time_ms)
        })
    }

    /// Builds the PQT2 for a beat grid, like `set_pmai_ext_ext_beat_position`.
    /// `sub_ms` is the per-beat body value (see the struct docs) and must have one entry per beat.
    pub fn from_beats(beats: &[Beat], sub_ms: Vec<u16>) -> Self {
        assert_eq!(beats.len(), sub_ms.len(), "one PQT2 entry per beat");
        Self {
            unk_0c: 0,
            unk_0e: 0,
            analysis_type: 1,
            unk_11: 0,
            unk_14: 0,
            first: beats.first().copied().unwrap_or_default(),
            last: beats.last().copied().unwrap_or_default(),
            checksum: Self::checksum_of(beats),
            tail: Some((0, 0)),
            entries: sub_ms,
        }
    }

    pub fn decode(s: &Section) -> TagResult<Self> {
        check_tag(s, &[Tag::PQT2])?;
        let mut h = Rd::new(s.tag, &s.header);
        let (unk_0c, unk_0e, analysis_type, unk_11) = (h.u16()?, h.u16()?, h.u8()?, h.u8()?);
        if h.u16()? != 2 {
            return h.err("unsupported PQT2 entry size");
        }
        let unk_14 = h.u32()?;
        let (first, last) = (Beat::read(&mut h)?, Beat::read(&mut h)?);
        let count = h.u32()? as usize;
        let checksum = h.u32()?;
        let tail = match h.remaining() {
            0 => None,
            8 => Some((h.u32()?, h.u32()?)),
            _ => return h.err("unsupported PQT2 header length"),
        };
        if count.checked_mul(2) != Some(s.body.len()) {
            return h.err("entry count does not match body");
        }
        let entries = s
            .body
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        Ok(Self {
            unk_0c,
            unk_0e,
            analysis_type,
            unk_11,
            unk_14,
            first,
            last,
            checksum,
            tail,
            entries,
        })
    }

    pub fn encode(&self) -> Section {
        let mut header = Vec::with_capacity(44);
        header.put_u16(self.unk_0c);
        header.put_u16(self.unk_0e);
        header.put_u8(self.analysis_type);
        header.put_u8(self.unk_11);
        header.put_u16(2);
        header.put_u32(self.unk_14);
        self.first.write(&mut header);
        self.last.write(&mut header);
        header.put_u32(self.entries.len() as u32);
        header.put_u32(self.checksum);
        if let Some((a, b)) = self.tail {
            header.put_u32(a);
            header.put_u32(b);
        }
        let body = self.entries.iter().flat_map(|v| v.to_be_bytes()).collect();
        Section {
            tag: Tag::PQT2,
            header,
            body,
        }
    }

    /// True when the header checksum matches `beats` (e.g. the PQTZ grid of the same track).
    pub fn matches(&self, beats: &[Beat]) -> bool {
        self.entries.len() == beats.len() && self.checksum == Self::checksum_of(beats)
    }
}
