# ANLZ analysis files

Status: **container verified**: 2,154 fixture files (stick + sandbox) round-trip byte-exact through `mb-anlz`. **Typed tags verified** for every tag that has fixtures: all 2,154 files also round-trip byte-exact through the typed views (`mb_anlz::tags`), with no section left raw. PSSI and PVDI layouts and both USB scrambles are **verified** on the oracle-a fixtures: 161/169 local sections, plus 157 scrambled USB copies that equal the local ones after unscrambling. `usb::to_usb` rebuilds all 471 exported DAT/EXT/2EX files byte-exact. Field *meanings* inside PSSI/PVDI are still partly hypothesis. The USB folder hash is **verified** on 704 of 704 stick sets.

Status per field in the tables below: **V** = verified (writer/setter code and fixture bytes agree), **C** = from code, fixtures constant or absent, **H** = hypothesis, **O** = open.

## Files
For each track rekordbox writes `ANLZ0000.DAT`, `.EXT` and `.2EX`. rekordbox 7 also writes `.3EX` locally; that file is MessagePack (see below), not PMAI.

| | Location |
|---|---|
| Local path | `<analysis-data-root>/PIONEER/USBANLZ/<UUID[0:3]>/<UUID[3:]>/ANLZ0000.*` |
| USB path | `PIONEER/USBANLZ/P<XXX>/<8 hex>/ANLZ000N.*` (`CreateAnlzFileFolderPath` @0x1007b2e18, format string `P%03X%s%08X` @0x103790b1c; see § Local vs USB) |

The stick has one set named `ANLZ0001`: folder `P046/00014454` holds two tracks whose different paths hash to the same folder, so the second set gets the next number.

## Container (big-endian)
| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic `PMAI` |
| 4 | 4 | header length (always 0x1C) |
| 8 | 4 | file length |
| 12 | 16 | PMAI header rest (below) |

PMAI header rest (`write_pmai_header` @0x100f5ad28, defaults from `initialize_pmai_header` @0x100f589b4, which copies 8 bytes from 0x1037c0730 and then stores a u64 of 1 at 0x14). Typed view: `mb_anlz::FileHeader`.

| Offset | Type | 7.2.19 value | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | C |
| 0x0e | u16 | 1 (0 in one older stick EXT) | V value, O meaning |
| 0x10 | u16 | 1 | V value, O meaning |
| 0x12 | u16 | 0 | C |
| 0x14 | u16 | 1 | V value, O meaning |
| 0x16 | u16 | 0 | C |
| 0x18 | u32 | 0 | C |

All 718 DAT, all 718 2EX and 717 of 718 EXT carry exactly these values. The meaning is open; likely version numbers.

Sections follow the header, each laid out as `tag(4) | header_len u32 | total_len u32 | tag-specific header | body`. Offsets in the tag tables below count from the tag. Unless a row says otherwise, numbers are big-endian. The writers build every field with `Write16`/`Write32`/`Write64` @0x101cc093c/0x101cc0948/0x101cc0954, which byte-swap to BE. Bodies that hold bytes (waveforms) are copied raw.

**Writer and validator in the binary**
- `analyzer::CMusicAnalyzeFile::write_pmai` @0x100f5ab40 (DAT), `write_pmai_ext` @0x1013d0230 (EXT), `write_pmai_ex2` @0x1007869f0 (2EX). Readers: `read_pmai` @0x100f5925c, `read_pmai_ext` @0x1013cdd3c, `read_pmai_ex2` @0x100785fc8.
- `write_pmai(info, true)` writes only the header and PPTH.
- `write_pmai_ext` keeps atoms it does not understand: `read_pmai_ext_unknown_atom` @0x1013cf638 collects them into a buffer that is appended after PSSI. A second raw buffer, info+0x48 with length at info+0x40, is written right after PWV3 (origin open). musicbox must preserve unknown EXT atoms in the same way.
- Tag/header-length tables (8 bytes per entry: tag + length): DAT @0x1037c0728 (PMAI 0x1c, PPTH 0x10, PVBR 0x10, PQTZ 0x18, PWAV 0x14, PWV2 0x14, PCOB 0x18); PCPT 0x1c / PCP2 0x10 @0x1037a0818; EXT @0x103790a30 (PWV3 0x18, PWV4 0x18, PQT2 0x38) and @0x1037d91d0 (PCO2 0x14, PWV5 0x18, PVB2 0x20, PSSI 0x20); 2EX @0x10378f9c0 (PWVC 0x0e, PWV6 0x14, PWV7 0x18, PVDI 0x18). The table @0x103790a48 holds PWV5/PVB2/PMAI with big-endian lengths, as used by the USB conversion.
- Validator: `PCF2::checkAtom` @0x101c6a8c4 dispatches to `checkAtomDAT` @0x101c6a9d8, `checkAtomEXT` @0x101c6afe8 and `checkAtom2EX` @0x101c6bcb4. These read big-endian ints and compare header lengths against the values above; the details are not needed for musicbox.

## Observed tag order and header lengths (fixtures, 2026-10-11)
| File | Writer order | Fixture order | Header lengths |
|---|---|---|---|
| DAT | PPTH PVBR PQTZ PWAV PWV2 PCOB(hot) PCOB(memory) | same | PPTH 16, PVBR 16, PQTZ 24, PWAV 20, PWV2 20, PCOB 24 |
| EXT | PPTH PWV3 [raw] PCOB(hot) PCOB(mem) PCO2(hot) PCO2(mem) PQT2 PWV5 PWV4 PVB2 PSSI [unknown atoms] | PPTH PWV3 PCOB PCOB PCO2 PCO2 [PQT2] PWV5 PWV4 [PVB2] | PWV3 24, PCO2 20, PQT2 56, PWV5 24, PWV4 24, PVB2 32, PSSI 32 |
| 2EX | PPTH PWV7 PWV6 PWVC PVDI | PPTH PWV7 PWV6 PWVC | PWV7 24, PWV6 20, PWVC 14, PVDI 24 |

- PQT2 is missing in 37 EXT files (sampler WAVs, and 2020-era stick files).
- PVB2 appears once.
- **Not yet observed:** PSSI (phrase) and PVDI (vocal). We need the oracle session with phrase and vocal analysis turned ON.
- The USB EXT export can also emit a `PKEY` atom (segmented key; tag built in `ExportAnalysisExtendWithCue`, data from `MstLoadSegmentedKey` @0x1007b6180). Layout **O**; no fixture.

## Tags
Typed views live in `crates/mb-anlz/src/tags/`. Each has `decode(&Section)` and `encode() -> Section`, and decode → encode is byte-identical for every fixture section. Fields whose meaning is unknown keep offset names (`unk_XX`). `TypedSection::decode` falls back to the raw section whenever a typed view would not reproduce the bytes.

### PPTH: track path (all files)
Writer `write_pmai_path` @0x100f5ae2c. View: `tags::Path`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | path length in bytes, including the NUL | V (2,154/2,154) |
| 0x10 | UTF-16BE | path, NUL-terminated | V |

Local files store `?/<audio file name>`; USB files store the device path `/Contents/…`. See § Local vs USB.

### PVBR: VBR index (.DAT)
Writer `write_pmai_vbr` @0x100f5af18; defaults in `initialize_pmai` (total 0x654). View: `tags::Vbr`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | from the struct; 0 in fixtures | C |
| 0x0e | u16 | written as constant 0 | V |
| 0x10 | u32 × 400 | index table; all zero in every fixture (CBR/AAC files) | V layout, O meaning |
| 0x650 | u32 | sample count (`set_pmai_number_of_sample` @0x101619de4 stores it at info+0x688 = this slot). 0 in 716 files, set in 2 local ones (e.g. 5,647,104) | H |

The writer accepts three body sizes: 1604 (table and count), 1600 (table only) or 4 (count only). Any other size is copied raw.

### PQTZ: beat grid (.DAT)
Writers `write_pmai_quantize` @0x100f5b0c8 and `write_pmai_quantize_data` @0x100f5b704 (which writes in chunks of 256 entries; no format effect). Setter `set_pmai_beat_position` @0x101619e88. View: `tags::BeatGrid`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | 0 | V |
| 0x10 | u16 | 8 (initialised from 0x1037c0750; likely the entry size) | V value, H meaning |
| 0x12 | i16 | beat adjust offset (setter's `short` argument, stored at quantize+0x12); 0 in fixtures | C |
| 0x14 | u32 | beat count | V |
| 0x18 | entry × count | beats, 8 bytes each | V |

Beat entry: `u16 beat_in_bar (1..4) | u16 tempo (BPM × 100) | u32 time (ms)`. Fixture checks: beat-to-beat spacing equals 60000/BPM within 2 ms in all 718 grids, and bar positions cycle 1→4. Eight stick grids have a single beat numbered 0 where a 4 is due, inside the last bar or two (meaning **O**).

### PWAV / PWV2: monochrome previews (.DAT)
Writers `write_pmai_wave400pt` @0x100f5b1c8 and `write_pmai_wave100pt` @0x100f5b2b8; defaults in `initialize_pmai`. View: `tags::WavePreview`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | column count: 400 (PWAV) / 100 (PWV2) | V |
| 0x10 | u16 | 1 | V value, O meaning |
| 0x12 | u16 | written as constant 0 | V |
| 0x14 | u8 × count | columns | V |

Column byte: low 5 bits are the height, high 3 bits the intensity (whiteness). This is the same packing as PWV3, where the export code shows it (see PWV3). Fixtures: PWAV heights reach 0..25 and intensity 0..5; PWV2 heights reach 0..15 and intensity is always 0. **H** for PWAV/PWV2 (layout inferred from PWV3 plus the value ranges).

### PCOB / PCPT: cue lists (.DAT and .EXT)
Writers `write_pmai_hot_cue` @0x100f5b3a8, `write_pmai_memory_cue` @0x100f5b558, `write_pmai_cue_data` @0x100f5b7fc (PCPT), and `write_pmai_ext_cue` @0x1013d06c4 for the EXT copy. Init: `initialize_pmai_hot_cue` @0x100f58a50 (room for 3 entries), `initialize_pmai_memory_cue` @0x100f58b08 (room for 10), `init_pmai_ext_cue` @0x1013cd4ac. View: `tags::CueList`.

PCOB header:

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | list kind: 1 = hot cues, 0 = memory cues (the hot-cue writer fails unless it is 1, the memory writer unless it is 0) | V |
| 0x10 | u16 | 0 | V |
| 0x12 | u16 | entry count | V |
| 0x14 | u16 | 0xffff in hot lists and empty lists. One memory list has `0000 0001`, which looks like the first/last index of the memory-cue linked list maintained by `add_pmai_mem_cue` @0x100f5bd70 | H |
| 0x16 | u16 | see 0x14 | H |

PCPT entry (sub-atom: header 0x1c, total 0x38). Fields are filled by `set_pmai_hot_cue` @0x100f5bbbc / `set_pmai_cue_point` @0x100f5bb64 from a `cue_point_t`:

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | hot cue number (1 = A …). 0 for memory cues; the .DAT hot list holds A–C only | V |
| 0x10 | u32 | `cue_point_t`+4; 0 in fixtures | C |
| 0x14 | u16 | status: the 7.x setter stores 1. 768 entries have 1, 290 (older stick files) have 0 | V value, H meaning |
| 0x16 | u16 | 0 | V |
| 0x18 | u16 | previous entry in the memory-cue list (0xffff = none) | H |
| 0x1a | u16 | next entry in the memory-cue list (0xffff = none) | H |
| 0x1c | u8 | cue type: 1 = point, 2 = loop | V |
| 0x1d | u8 | written as constant 0 | V |
| 0x1e | u16 | time unit, 1000 (positions are ms; `convertToMsec` @0x100f5bb1c treats 1000 as identity, 0 as "position 0", and any other value as positions per second, rounded up) | V |
| 0x20 | u32 | position (ms) | V |
| 0x24 | u32 | loop end (ms), 0xffffffff = none | V |
| 0x28 | u32 × 4 | 0 when status = 1, 0xffffffff otherwise | V value, O meaning |

Fixture check: each DAT hot cue reappears in the EXT PCO2 hot list with the same number, position and type.

### PWV3: monochrome scrolling waveform (.EXT)
Writer `write_pmai_ext_zoom_wave` @0x1013d0554, init `init_pmai_ext_zoom_wave` @0x1013cd59c. View: `tags::WaveScroll`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | entry size, 1 | V |
| 0x10 | u32 | column count | V |
| 0x14 | u16 | columns per second, 150 (one column per 1/150 s) | V |
| 0x16 | u16 | 0 | V |
| 0x18 | u8 × count | columns | V |

Column byte = `height | intensity << 5`: height in bits 0–4 (0..31), intensity in bits 5–7 (0..7). The USB export packs it from two separate values (`ExportAnalysisExtendWithCue` @0x1007b83fc, around 0x1007b90a8), and fixture values match (heights up to 31, intensity up to 7).

### PCO2 / PCP2: extended cue lists (.EXT)
Writers `write_pmai_ext_cue_ext` @0x1013d087c and `write_pmai_ext_cue_ext_data` @0x1013d1340; builder `add_pmai_ext_cue_ext_point` @0x1013d1960; comment setter `set_pmai_ext_cue_comment` @0x1013d1bc8; init `init_pmai_ext_cue_ext_info` @0x1013cd4d4. View: `tags::CueListExt`.

PCO2 header:

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | list kind: 1 = hot, 0 = memory | V |
| 0x10 | u16 | entry count | V |
| 0x12 | u16 | 0 | V |

PCP2 entry (sub-atom header 0x10; total = 0x2c + comment bytes + extension bytes):

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | hot cue number (1 = A …; up to 8 = H in fixtures), 0 for memory cues | V |
| 0x10 | u8 | cue type: 1 = point, 2 = loop | V |
| 0x11 | u8 | 0 | V |
| 0x12 | u16 | time unit, 1000 | V |
| 0x14 | u32 | position (ms) | V |
| 0x18 | u32 | loop end (ms), 0xffffffff = none | V |
| 0x1c | u8 | memory-cue colour: the builder stores colour index + 1 when the index is 0..7, else 0. 0 in all fixtures | C |
| 0x1d | u8 | from `cue_ext_data_t`+9. 1 in every 88-byte entry, 0 in 48-byte entries, 3 once | O |
| 0x1e | u16 | from `cue_ext_data_t`+0x0e. Values 0..995: sub-millisecond part of the position in µs | H |
| 0x20 | u16 | from `cue_ext_data_t`+0x10. 494 on the only loop with an end: sub-ms part of the loop end | H |
| 0x22 | u16 | 0 | C |
| 0x24 | u16 | beat-loop numerator (stored only when numerator and denominator are both non-zero; one fixture loop has 8/1 here) | C |
| 0x26 | u16 | beat-loop denominator | C |
| 0x28 | u16 | 0 | C |
| 0x2a | u16 | comment length in bytes, including the NUL; 0 = no comment. The builder rejects comments shorter than 4 bytes or longer than 0xffff | C (no fixture comment) |
| 0x2c | UTF-16BE | comment, NUL-terminated | C |
| … | 0 / 4 / 44 bytes | extension | V |

Extension: the builder writes 44 bytes if `cue_ext_data_t`+0x38 is non-zero, 4 bytes if any of the four colour bytes is non-zero, and none otherwise.
- First 4 bytes: hot-cue colour code, R, G, B. Fixture pairs: code 8 → 00c4ff, 19 → 00ff30, 23 → 33ff00, 32 → ffe800, 36 → ff8c00, 38 → ff5e00, 43 → ff0017, 60 → 4d00ff. 48-byte entries carry code 0 with an RGB value.
- The 44-byte form adds four u64 and two u32 after the colour bytes, all zero in fixtures (meaning **O**).
- The writer also appends a raw trailing buffer (entry+0x78, length at +0x70) after the extension. The typed view keeps everything after the comment as raw `ext` bytes.

Fixture entry lengths: 849 × 88, 421 × 48, 3 × 44. `ExportAnalysisExtendWithCue` appears to size every exported entry as 0x58 + comment (adds at 0x1007b8918/0x1007b8a74), so the 48- and 44-byte entries probably come from older exports.

### PQT2: extended beat grid (.EXT)
Writer `write_pmai_ext_quantize_ext` @0x1013d09a0, setters `set_pmai_ext_ext_beat_position` @0x100f2667c and `set_pmai_ext_beat_num_offset` @0x100f2683c, init `init_pmai_ext_quantize_ext` @0x1013cd5fc. View: `tags::ExtBeatGrid`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | 0 | V |
| 0x10 | u8 | beat analysis type (`get_pmai_ext_beat_analysis_type`); 1 in fixtures, init 0 | V value, O meaning |
| 0x11 | u8 | 0 | V |
| 0x12 | u16 | entry size, 2 | V |
| 0x14 | u32 | setter stores 0 | V |
| 0x18 | beat (8) | first beat, same layout as a PQTZ entry | V (681/681 equal PQTZ[0]) |
| 0x20 | beat (8) | last beat | V (681/681 equal the last PQTZ beat) |
| 0x28 | u32 | entry count (= PQTZ beat count) | V |
| 0x2c | u32 | checksum: wrapping sum over all beats of `beat_in_bar + tempo + time_ms` | V (681/681) |
| 0x30 | u32 | beat number offset (`set_pmai_ext_beat_num_offset`); written only when the header is 0x38 long; 0 in fixtures | C |
| 0x34 | u32 | 0 | C |
| 0x38 | u16 × count | per-beat value: the setter copies the u16 at offset 8 of each 12-byte `DetailedBeatInfo`. Range 0..997 in fixtures: sub-millisecond part of the beat time in µs | H |

### PWV5: colour scrolling waveform (.EXT)
Writer `write_pmai_ext_new_zoom_wave` @0x1013d0c40, init `init_pmai_ext_new_zoom_wave` @0x1013cd5d8. View: `tags::WaveColorScroll`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | entry size, 2 | V |
| 0x10 | u32 | column count (equals the PWV3 count in 707 of 718 files) | V |
| 0x14 | u16 | columns per second, 150 | V |
| 0x16 | u8 | 3: bits per colour channel | V value, H meaning |
| 0x17 | u8 | 5: bits of height | V value, H meaning |
| 0x18 | u16 × count | columns | V |

Column u16 = `r << 13 | g << 10 | b << 7 | height << 2`: three 3-bit channels and a 5-bit height. Bits 0–1 are zero (all 44 million fixture columns). The packing is in `ExportAnalysisExtendWithCue` around 0x1007b95d4, which builds it from 4-byte `ColorZoomWaveInfo` values (r, g, b, height).

### PWV4: colour preview (.EXT)
Writer `write_pmai_ext_new_whole_wave` @0x1013d0dcc, init `init_pmai_ext_new_whole_wave` @0x1013cd5bc. View: `tags::WaveColorPreview`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | entry size, 6 | V |
| 0x10 | u32 | column count, 1200 | V |
| 0x14 | u16 | 0 | V |
| 0x16 | u16 | 0 | V |
| 0x18 | 6 bytes × count | columns | V layout |

Column bytes: fixtures show bytes 0–1 using 0..255 and bytes 2–5 using 0..127. `djplay::UiPlayer::deviceUpdateWholeWaveform` @0x10202d088 renders 6-byte whole-wave points (1200 per track). It merges two points per column (per-byte max), then:
- **RGB style:** the colour is (b3, b4, b5) scaled so the largest is 255 (white if all are 0); the height is max(b3, b4, b5) and the secondary height is b5. All heights are scaled by 127 / (track-wide max of b3..b5).
- **Single-colour style:** the colour comes from `getWaveformBlueColour(b0, b1, b2)` @0x102034cd4.
- **3-band style:** b3, b4, b5 are passed straight to `add3BandWholeWaveformPosition`.

So b3..b5 look like three band amplitudes and b0..b2 like colour/brightness inputs. **H**: the renderer is the device-data path, and it is not proven to read the PWV4 bytes. `WaveColorPreview::rgb_column` implements the RGB style.

### PVB2: seek table (.EXT)
Writer `write_pmai_ext_seek_table` @0x1013d0f40, setter `set_pmai_ext_seek_table_data` @0x100f26928 (args: u64, count, data), init @0x1013cd62c, `MstStoreSeekTable` @0x1007b761c. One fixture (an MP3 on the stick). View: `tags::SeekTable`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | C |
| 0x0e | u16 | 0 | C |
| 0x10 | u64 | total samples (fixture 11,746,116 ≈ 266 s at 44.1 kHz) | H |
| 0x18 | u32 | entry count (400) | C |
| 0x1c | u32 | entry size, 20 (init constant) | C |
| 0x20 | entry × count | `u64 sample position, u64 byte offset in the audio file, u32 samples per block` (fixture steps of 27,648 samples, blocks of 4,608 = 4 MP3 frames) | H |

### PSSI: song structure / phrases (.EXT), no fixtures
Writer `write_pmai_ext_song_struct` @0x1013d10b4, init `init_pmai_ext_song_struct` @0x1013cd654, setters `set_pmai_ext_song_struct_info` @0x100f27244 and `set_pmai_ext_song_struct_data` @0x100f26a7c. View: `tags::SongStructure` (decodes scrambled and plain forms; `scrambled` records which one was stored).

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | written as constant 0; also the scramble's start index | C |
| 0x0e | u16 | entry size, 24 (constant) | C |
| 0x10 | u16 | phrase count; its low byte keys the scramble | C |
| 0x12 | u16 | mood (from the struct; first scrambled byte) | H name |
| 0x14 | u32 | written as constant 0 | C |
| 0x18 | u16 | written as constant 0 (scramble detector, see below) | C |
| 0x1a | u16 | end beat (from the struct) | H name |
| 0x1c | u8 | bank (from the struct) | H name |
| 0x1d | u8 | from the struct | O |
| 0x1e | u8 | from the struct | O |
| 0x1f | u8 | local writer: constant 0; the USB export copies a value here | O |
| 0x20 | entry × count | phrases, 24 bytes each, written as 12 BE u16 words | C |

The phrase word names are community hints that we have not checked: 0 index, 1 start beat, 2 kind (depends on the mood), 10 fill flag, 11 fill start beat. **H**.

### PWV7 / PWV6: 3-band waveforms (.2EX)
Writers `write_pmai_ex2_3bands_zoom_wave` @0x100786bb4 (PWV7) and `write_pmai_ex2_3bands_whole_wave` @0x100786d28 (PWV6); init @0x100785e68 / @0x100785e48. View: `tags::Wave3Band`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 | entry size, 3 | V |
| 0x10 | u32 | column count (PWV6 1200; PWV7 equals the PWV3 count in 707 of 718 sets) | V |
| 0x14 | u16 | PWV7 only: columns per second, 150 | V |
| 0x16 | u16 | PWV7 only: 0 | V |
| … | 3 bytes × count | columns | V layout |

Three bytes per column, read as the low/mid/high band amplitudes (**H**; `add3BandWholeWaveformPosition` takes three such bytes). Fixture ranges: PWV7 0..127, PWV6 up to 88/91/100. The USB export (`ExportAnalysisExtend2`) copies PWV6 with 1200 points and PWV7 unchanged.

### PWVC: waveform normalisation (.2EX)
Writer `write_pmai_ex2_wave_normalize_coef` @0x100786e6c; defaults in the table @0x10378f9c0 (0, then 100, 100, 100). The header is 0x0e long, so the body starts at 0x0e. View: `tags::WaveNormalize`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | 0 | V |
| 0x0e | u16 × 3 | coefficients (`WaveNormalizeCoefInfo`). Fixtures: the first is 80 in 616 of 718 files (range 80..300); the second and third are mostly 100..150 (range 80..500) | V layout, H meaning (per-band gain in percent) |

### PVDI: vocal detection (.2EX), no fixtures
Writer `write_pmai_ex2_vocal_detect` @0x100786f44, init `init_pmai_ex2_vocal_detect` @0x100785e88 (defaults from 0x10378f9e8), version get/set @0x100eaa0c8 / @0x100eaa148. View: `tags::VocalDetect`.

| Offset | Type | Field | Status |
|---|---|---|---|
| 0x0c | u16 | version; bit 15 set = body scrambled (USB) | C |
| 0x0e | u16 | default 1024 (hop size?) | C value, H meaning |
| 0x10 | u16 | default 22050 (sample rate?) | C value, H meaning |
| 0x12 | u16 | entry size, default 1 | C |
| 0x14 | u32 | entry count | C |
| 0x18 | bytes | count × entry size; one byte per point | C |

## Local vs USB
USB files are rebuilt, not copied:
- `ExportAnalysisData` @0x1007b7c0c builds the DAT from the local analysis (`MstLoadVBR`, `MstLoadDotWave`, `MstLoadLoudWave`), the passed beat grid (`BeatPositionSet`) and the passed hot/memory cue arrays (`USB_CUE_Data`, swapped by `ConvUSB_CUEPoint`).
- `ExportAnalysisExtendWithCue` @0x1007b83fc does the same for the EXT. It reloads PWV3/PWV5/PWV4/PVB2/PSSI from the local file (`MstLoadZoomWave`, `MstLoadColorZoomWave`, `MstLoadWholeWave`, `MstLoadSeekTable`, `MstLoadSongStruct`, `MstLoadSegmentedKey`), takes the cues and the extended beat grid from its arguments, and repacks everything big-endian.
- `ExportAnalysisExtend2` @0x1007b9a04 copies the 2EX through the normal setters.

Differences that matter for interoperable exports:

| Item | Local | USB | Status |
|---|---|---|---|
| PPTH | `?/` + audio file name (`get_new_format_ppth` @0x100656bac: prefix `s_new_ppth_prefix` @0x1056c7378, `/`, then `File::getFileName`). Streaming tracks map `/v4/catalog/tracks/` → `beatport:tracks:` and `/bs/catalog/tracks/` → `beatsource:tracks:` | device path, e.g. `/Contents/<artist>/<album>/<file>` | V (14 local, 704 USB sets; the `?` prefix value is from fixtures) |
| Cues (PCOB/PCO2) | written when cues are edited; the sandbox fixtures have none | from the database cue rows at export time | V structure |
| PSSI | plain | scrambled (below) | C |
| PVDI | plain, version bit 15 clear | body scrambled, version \|= 0x8000 | C |
| PKEY | not written | written by the EXT export when segmented-key data exists | O |

### USB folder hash (`CreateAnlzFileFolderPath` @0x1007b2e18)
1. Take the track's device path, the same string as USB PPTH (`DataSource` vfunc +0x28), as UTF-16 code units.
2. `h = 0`. For each unit `c`: `h = h * 0x5bc9 + c`, then `h = h * 0x93b5 + c` (u32 wrapping).
3. `h = h mod 200003` (0x30d43; the compiler emits a multiply by 0xa7c5075b and a shift by 49).
4. Group `g` (7 bits) = bits 0, 2, 6, 7, 9, 13, 16 of `h`, packed into bits 0..6.
5. Folder = `sprintf("P%03X/%08X", g, h)`, under `PIONEER/USBANLZ/`. The function uses `.PIONEER` instead when both of its trailing bool arguments are true (their meaning is open).
6. When the `unsigned int` argument is non-zero it takes the local branch instead: `<uuid[0:3]>/<uuid[3:]>` from `DataSource` vfunc +0x98.
7. On a collision the file number increments (`ANLZ0001`).

Verified: all 704 stick sets (703 folders) match the hash of their PPTH path; `usb_folder_hash_matches_stick` in `crates/mb-anlz/tests/roundtrip.rs`. Code: `mb_anlz::usb::{path_hash, usb_anlz_folder, usb_anlz_dir, local_anlz_folder, local_ppth}`.

### PSSI scramble (`ConvUSB_ScrambleSongStruct` @0x1007b3890)
- **Key:** the 19 ASCII bytes `Kanzen-niRikaishita` @0x104f91ec0. It is mixed with `c = section[0x11]` (low byte of the phrase count): `k[i] = (KEY[i] + c) ^ 0x80`.
- **Range:** with `total` = BE u32 at section+8, and only if `total >= 19`, bytes `0x12 .. total` are XORed with `k`. The key index starts at `BE u16 @0x0c mod 19` (0 from the writer) and cycles.
- **When applied:** the export scrambles only if bytes 0x18 and 0x19 are both zero. They are zero in plain form, and after scrambling they become `k[6]`, `k[7]`, which cannot both be zero. So "u16 @0x18 != 0" means the section is scrambled.
- **Involution:** the XOR is its own inverse, and the key inputs (0x0c, 0x11) lie outside the scrambled range.

Code: `mb_anlz::usb::scramble_song_structure`, which works on the whole section bytes.

### PVDI scramble (`ConvUSB_ScrambleVocalDetect` @0x1007b3b3c; caller `ExportAnalysisExtend2`)
- **Key:** the same 19-byte key. The function formats `"%d\n"` with the constant 0 into a 17-byte buffer, which gives the seed bytes `'0'` (0x30) and `'\n'` (0x0a).
- **Mix:** `k[i] = (KEY[i] + 0x30) ^ 0x80`.
- **Start index:** `(0x30 + 0x0a * 256) mod 19` = 5.
- **Range:** XORs the `point_num` entry bytes, cycling the key.
- **Condition:** applied only when the version is non-negative as an i16; afterwards the version gets bit 15.

Code: `mb_anlz::usb::scramble_vocal_detect`, unit-tested for involution and the start index. **H** until a fixture with vocal data exists.

## Open questions
- PMAI header values 0x0e/0x10/0x14 (version numbers?) and the one EXT with 0x0e = 0.
- PQTZ entries with `beat_in_bar` = 0 (8 stick files, near the end).
- PQT2 per-beat u16 and PCP2 0x1e/0x20: µs remainders? Needs a track with known sub-ms positions.
- PCP2 0x1d and the 44-byte extension tail; the raw buffers after PWV3 and in PCP2.
- PCOB 0x14/0x16 and PCPT prev/next semantics (need a library with several memory cues).
- PWV4 byte meaning (b0..b2): confirm with the analysis writer or a rendering diff.
- PVB2 fields (one fixture).
- PSSI/PVDI on real data: needs the oracle session with phrase and vocal analysis on. Then check `SongStructure` and the scrambles against rekordbox's own USB export.
- `PKEY` layout; meaning of the `CreateAnlzFileFolderPath` bools.

## .3EX (local only, rekordbox 7)
- A MessagePack map `{"embedding": {...}}`. Keys d2…d12 hold the content ID, the audio MD5, the duration, vectors and the BPM.
- Produced by the ONNX embed models. Optional (M6.8).
