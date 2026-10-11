//! Typed views on synthetic sections (no fixtures needed), mainly for PSSI and PVDI, which no
//! fixture contains yet, and for fields that are constant in the fixtures (comments, colours).

use mb_anlz::tags::*;
use mb_anlz::{AnlzFile, FileHeader, Section, Tag, usb};

fn round_trip(t: TypedSection) -> TypedSection {
    let s = t.encode();
    let back = TypedSection::try_decode(&s).expect("decodes");
    assert_eq!(back, t);
    assert_eq!(back.encode(), s);
    back
}

fn song_structure(scrambled: bool) -> SongStructure {
    let phrase = |i: u16, beat: u16| Phrase {
        words: [i, beat, 2, 0, 0, 0, 0, 0, 0, 0, 1, beat + 28],
    };
    SongStructure {
        scrambled,
        unk_0c: 0,
        mood: 1,
        unk_14: 0,
        unk_18: 0,
        end_beat: 400,
        bank: 3,
        unk_1d: 0,
        unk_1e: 0,
        unk_1f: 0,
        phrases: vec![phrase(1, 1), phrase(2, 33), phrase(3, 65)],
    }
}

#[test]
fn pssi_plain_and_scrambled_round_trip() {
    let plain = song_structure(false);
    let plain_section = plain.encode();
    assert!(!SongStructure::is_scrambled(&plain_section));
    round_trip(TypedSection::SongStructure(plain.clone()));

    let scrambled = song_structure(true);
    let stored = scrambled.encode();
    assert!(SongStructure::is_scrambled(&stored));
    // Prefix and count stay readable; everything from 0x12 on differs from the plain form.
    assert_eq!(stored.header[..6], plain_section.header[..6]);
    assert_ne!(stored.body, plain_section.body);
    let decoded = SongStructure::decode(&stored).unwrap();
    assert_eq!(decoded.phrases, plain.phrases);
    assert_eq!((decoded.mood, decoded.end_beat, decoded.bank), (1, 400, 3));
    round_trip(TypedSection::SongStructure(scrambled));
}

#[test]
fn pssi_scramble_matches_the_section_level_function() {
    let plain = song_structure(false).encode();
    let mut bytes = AnlzFile {
        header: FileHeader::default().encode(),
        sections: vec![plain],
    }
    .to_bytes();
    let section = &mut bytes[0x1c..];
    usb::scramble_song_structure(section);
    let file = AnlzFile::parse(&bytes).unwrap();
    assert_eq!(file.sections[0], song_structure(true).encode());
}

#[test]
fn pvdi_plain_and_scrambled_round_trip() {
    let data: Vec<u8> = (0..200u8).map(|i| i % 3).collect();
    let plain = VocalDetect {
        version: 1,
        unk_0e: 1024,
        unk_10: 22050,
        entry_size: 1,
        data: data.clone(),
    };
    let s = plain.encode();
    assert_eq!(s.body, data);
    round_trip(TypedSection::VocalDetect(plain));

    let scrambled = VocalDetect {
        version: 1 | 0x8000,
        unk_0e: 1024,
        unk_10: 22050,
        entry_size: 1,
        data: data.clone(),
    };
    let s = scrambled.encode();
    assert_ne!(s.body, data);
    let mut unscrambled = s.body.clone();
    usb::scramble_vocal_detect(&mut unscrambled);
    assert_eq!(unscrambled, data);
    assert_eq!(VocalDetect::decode(&s).unwrap().data, data);
    round_trip(TypedSection::VocalDetect(scrambled));
}

#[test]
fn pcp2_comment_and_colour_round_trip() {
    let entry = CueEntryExt {
        unk_0c: 0,
        hot_cue: 2,
        cue_type: CueType::Loop,
        unk_11: 0,
        time_unit: 1000,
        time: 12_345,
        loop_end: 20_000,
        color_index: 0,
        unk_1d: 1,
        in_us: 250,
        out_us: 999,
        unk_22: 0,
        loop_numerator: 4,
        loop_denominator: 1,
        unk_28: 0,
        comment: Some("Drop ♪".into()),
        ext: [vec![0x2b, 0xff, 0x00, 0x17], vec![0; 40]].concat(),
    };
    assert_eq!(entry.hot_cue_letter(), Some('B'));
    assert_eq!(entry.color(), Some((0x2b, [0xff, 0x00, 0x17])));
    let list = CueListExt {
        unk_0c: 0,
        kind: CueKind::Hot,
        unk_12: 0,
        entries: vec![entry],
    };
    let s = list.encode();
    // 0x2c fixed + "Drop ♪\0" (7 units = 14 bytes) + 44 ext bytes.
    assert_eq!(s.body.len(), 0x2c + 14 + 44);
    assert_eq!(u16::from_be_bytes([s.body[0x2a], s.body[0x2b]]), 14);
    round_trip(TypedSection::CueListExt(list));
}

#[test]
fn pcob_and_cue_helpers() {
    let entry = CueEntry {
        unk_0c: 0,
        hot_cue: 0,
        unk_10: 0,
        status: 0,
        unk_16: 0,
        prev: 0xffff,
        next: 0xffff,
        cue_type: CueType::Point,
        unk_1d: 0,
        time_unit: 1000,
        time: 165_000,
        loop_end: 0xffff_ffff,
        unk_28: [0xffff_ffff; 4],
    };
    assert_eq!((entry.time_ms(), entry.loop_end_ms()), (165_000, None));
    let mut list = CueList::empty(CueKind::Memory);
    list.entries.push(entry);
    let s = list.encode();
    assert_eq!(s.header, [0, 0, 0, 0, 0, 0, 0, 1, 0xff, 0xff, 0xff, 0xff]);
    assert_eq!(s.body.len(), 56);
    round_trip(TypedSection::CueList(list));
}

#[test]
fn colour_column_packing() {
    let c = ColorColumn {
        red: 7,
        green: 3,
        blue: 1,
        height: 31,
    };
    assert_eq!(c.raw(), (7 << 13) | (3 << 10) | (1 << 7) | (31 << 2));
    assert_eq!(ColorColumn::from_raw(c.raw()), c);
    assert_eq!(c.rgb8(), [255, 109, 36]);
    let p = PreviewColumn {
        height: 25,
        intensity: 5,
    };
    assert_eq!(PreviewColumn::from_byte(p.to_byte()), p);
}

#[test]
fn ext_beat_grid_from_beats() {
    let beats: Vec<Beat> = (0..8)
        .map(|i| Beat {
            beat_in_bar: i % 4 + 1,
            tempo: 12000,
            time_ms: 25 + 500 * u32::from(i),
        })
        .collect();
    let pqt2 = ExtBeatGrid::from_beats(&beats, vec![0; 8]);
    assert!(pqt2.matches(&beats));
    let s = pqt2.encode();
    assert_eq!(s.header.len(), 0x38 - 12);
    assert_eq!(s.body.len(), 16);
    round_trip(TypedSection::ExtBeatGrid(pqt2));
    round_trip(TypedSection::BeatGrid(BeatGrid::new(beats)));
}

#[test]
fn malformed_sections_stay_raw() {
    // PPTH whose length field disagrees with the body.
    let bad = Section {
        tag: Tag::PPTH,
        header: vec![0, 0, 0, 9],
        body: vec![0, b'a', 0, 0],
    };
    assert!(TypedSection::try_decode(&bad).is_err());
    assert_eq!(
        TypedSection::decode(&bad),
        TypedSection::Unknown(bad.clone())
    );
    // Unknown tag (e.g. PKEY, written by the USB export but not in any fixture).
    let pkey = Section {
        tag: Tag(*b"PKEY"),
        header: vec![1, 2],
        body: vec![3],
    };
    assert!(TypedSection::decode(&pkey).is_unknown());
}
