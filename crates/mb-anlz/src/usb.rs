//! USB-export specifics: the `USBANLZ/Pxxx/xxxxxxxx` folder hash and the PSSI/PVDI scrambles.
//! Spec: `docs/re/anlz.md` § Local vs USB.

/// Key shared by both scrambles (19 ASCII bytes @0x104f91ec0).
pub const SCRAMBLE_KEY: &[u8; 19] = b"Kanzen-niRikaishita";

/// The modulus of the folder hash.
pub const FOLDER_HASH_MODULUS: u32 = 200_003;

/// Hash of a track path as `CreateAnlzFileFolderPath` @0x1007b2e18 computes it, over the UTF-16
/// code units of the device path (the same string that goes into PPTH, e.g. `/Contents/a/b.mp3`).
pub fn path_hash(device_path: &str) -> u32 {
    let mut h: u32 = 0;
    for unit in device_path.encode_utf16() {
        let c = u32::from(unit);
        h = h.wrapping_mul(0x5bc9).wrapping_add(c);
        h = h.wrapping_mul(0x93b5).wrapping_add(c);
    }
    h % FOLDER_HASH_MODULUS
}

/// The 7-bit `Pxxx` number: bits 0, 2, 6, 7, 9, 13 and 16 of the hash, packed low to high.
pub fn folder_group(hash: u32) -> u32 {
    (hash & 1)
        | (hash >> 1 & 0x2)
        | (hash >> 4 & 0x4)
        | (hash >> 4 & 0x8)
        | (hash >> 5 & 0x10)
        | (hash >> 8 & 0x20)
        | (hash >> 10 & 0x40)
}

/// `P%03X/%08X` for a device path, e.g. `P046/00014454`. Verified on all 704 stick sets.
pub fn usb_anlz_folder(device_path: &str) -> String {
    let h = path_hash(device_path);
    format!("P{:03X}/{:08X}", folder_group(h), h)
}

/// The analysis directory relative to the device root: `PIONEER/USBANLZ/Pxxx/xxxxxxxx`.
/// `CreateAnlzFileFolderPath` uses `.PIONEER` instead when both of its trailing bool arguments
/// are set (their meaning is open); pass `hidden_root` for that variant.
pub fn usb_anlz_dir(device_path: &str, hidden_root: bool) -> String {
    let root = if hidden_root { ".PIONEER" } else { "PIONEER" };
    format!("{root}/USBANLZ/{}", usb_anlz_folder(device_path))
}

/// Local analysis folder below `.../PIONEER/USBANLZ/`: `<uuid[0..3]>/<uuid[3..]>` (the branch of
/// `CreateAnlzFileFolderPath` taken for local files).
pub fn local_anlz_folder(content_uuid: &str) -> String {
    let split = content_uuid
        .char_indices()
        .nth(3)
        .map_or(content_uuid.len(), |(i, _)| i);
    format!("{}/{}", &content_uuid[..split], &content_uuid[split..])
}

/// PPTH text rekordbox 7 writes into local analysis files: `?/` + the audio file name
/// (`get_new_format_ppth` @0x100656bac; the `?` prefix is observed in every local fixture).
pub fn local_ppth(file_name: &str) -> String {
    format!("?/{file_name}")
}

fn keyed(mix: u8) -> [u8; 19] {
    SCRAMBLE_KEY.map(|k| k.wrapping_add(mix) ^ 0x80)
}

/// XOR-scrambles a whole PSSI section in place (tag through end of body), exactly like
/// `ConvUSB_ScrambleSongStruct` @0x1007b3890. Being an XOR it also unscrambles.
///
/// Key byte `i` = `(KEY[i] + section[0x11]) ^ 0x80` (0x11 = low byte of the phrase count). Bytes
/// 0x12..total_len (total from the BE u32 at 0x08) are XORed with the key, starting at key index
/// `BE u16 @0x0c mod 19` and cycling. Sections shorter than 19 bytes are left alone.
pub fn scramble_song_structure(section: &mut [u8]) {
    if section.len() < 0x12 {
        return;
    }
    let total = u32::from_be_bytes(section[8..12].try_into().unwrap()) as usize;
    if total < SCRAMBLE_KEY.len() {
        return;
    }
    let key = keyed(section[0x11]);
    let mut idx = usize::from(u16::from_be_bytes([section[0x0c], section[0x0d]])) % key.len();
    let end = total.min(section.len());
    for b in &mut section[0x12..end] {
        *b ^= key[idx];
        idx = (idx + 1) % key.len();
    }
}

/// XOR-scrambles PVDI entry bytes in place, exactly like `ConvUSB_ScrambleVocalDetect` @0x1007b3b3c.
/// Being an XOR it also unscrambles.
///
/// The function formats `"%d\n"` with the constant 0, giving the seed bytes `'0'` and `'\n'`:
/// key byte `i` = `(KEY[i] + 0x30) ^ 0x80`, start index = `(0x30 + 0x0a * 256) % 19` = 5.
pub fn scramble_vocal_detect(data: &mut [u8]) {
    let seed = *b"0\n";
    let key = keyed(seed[0]);
    let mut idx = (usize::from(seed[0]) + (usize::from(seed[1]) << 8)) % key.len();
    for b in data {
        *b ^= key[idx];
        idx = (idx + 1) % key.len();
    }
}


/// Builds the USB copy of a local analysis file, as `ExportAnalysisData*` does: PPTH becomes the
/// device path (`/Contents/…`), PSSI and PVDI are stored scrambled, every other section is copied
/// unchanged. Verified byte-exact against rekordbox's own export (`rbdiff anlz-export`, oracle-a:
/// 157 tracks × DAT/EXT/2EX).
pub fn to_usb(local: &crate::AnlzFile, device_path: &str) -> crate::AnlzFile {
    use crate::tags::{Path, SongStructure, VocalDetect};
    let sections = local
        .sections
        .iter()
        .map(|s| match &s.tag.0 {
            b"PPTH" => match Path::decode(s) {
                Ok(mut p) => {
                    p.path = device_path.to_owned();
                    p.encode()
                }
                Err(_) => s.clone(),
            },
            b"PSSI" => match SongStructure::decode(s) {
                Ok(mut p) => {
                    p.scrambled = true;
                    p.encode()
                }
                Err(_) => s.clone(),
            },
            b"PVDI" => match VocalDetect::decode(s) {
                Ok(mut v) => {
                    v.version |= 0x8000;
                    v.encode()
                }
                Err(_) => s.clone(),
            },
            _ => s.clone(),
        })
        .collect();
    crate::AnlzFile { header: local.header.clone(), sections }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_hash_examples() {
        assert_eq!(path_hash(""), 0);
        assert_eq!(usb_anlz_folder(""), "P000/00000000");
        // Every result stays below the modulus and the group fits in 7 bits.
        for p in ["/Contents/a.mp3", "/Contents/Ünïcødé/♪.flac"] {
            assert!(path_hash(p) < FOLDER_HASH_MODULUS);
            assert!(folder_group(path_hash(p)) < 0x80);
        }
    }

    #[test]
    fn local_forms() {
        assert_eq!(local_anlz_folder("149a1b2c-3d4e"), "149/a1b2c-3d4e");
        assert_eq!(local_ppth("Demo Track 2.mp3"), "?/Demo Track 2.mp3");
    }

    #[test]
    fn song_structure_scramble_is_an_involution_and_spares_the_prefix() {
        // PSSI header (0x20) + two zero phrases; count = 2.
        let mut section = vec![0u8; 0x20 + 48];
        section[..4].copy_from_slice(b"PSSI");
        section[4..8].copy_from_slice(&0x20u32.to_be_bytes());
        section[8..12].copy_from_slice(&((0x20 + 48) as u32).to_be_bytes());
        section[0x0e..0x10].copy_from_slice(&24u16.to_be_bytes());
        section[0x10..0x12].copy_from_slice(&2u16.to_be_bytes());
        section[0x12..0x14].copy_from_slice(&3u16.to_be_bytes()); // mood
        let plain = section.clone();
        scramble_song_structure(&mut section);
        assert_eq!(section[..0x12], plain[..0x12]);
        assert_ne!(section, plain);
        // First scrambled byte: 0x00 ^ ((b'K' + 2) ^ 0x80), mood high byte at key index 0.
        assert_eq!(section[0x12], (b'K' + 2) ^ 0x80);
        // 0x18/0x19 are never both zero after scrambling plain zeros.
        assert_ne!(section[0x18..0x1a], [0, 0]);
        scramble_song_structure(&mut section);
        assert_eq!(section, plain);
    }

    #[test]
    fn vocal_scramble_is_an_involution_with_start_index_5() {
        let plain: Vec<u8> = (0..64).collect();
        let mut data = plain.clone();
        scramble_vocal_detect(&mut data);
        assert_eq!(
            data[0],
            plain[0] ^ (SCRAMBLE_KEY[5].wrapping_add(0x30) ^ 0x80)
        );
        assert_eq!(
            data[14],
            plain[14] ^ (SCRAMBLE_KEY[0].wrapping_add(0x30) ^ 0x80)
        );
        scramble_vocal_detect(&mut data);
        assert_eq!(data, plain);
    }
}
