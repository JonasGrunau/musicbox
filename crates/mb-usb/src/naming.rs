//! Device paths under `/Contents/` and the ANLZ / artwork paths derived from them, exactly as
//! rekordbox 7.2.19 builds them. Spec: `docs/re/export.md` § 1.3, § 1.4, § 1.6, § 3.
//!
//! Everything here is pure string work; whether a name is taken is decided by the caller (the
//! planner keeps the set of paths that exist on the device or were planned earlier).

use unicode_normalization::UnicodeNormalization;

/// Longest directory or file name rekordbox writes, in Unicode code points after NFC
/// (`String::append(s, 48)` in `get_device_file_path_candidate` @0x1009ba39c).
pub const MAX_COMPONENT: usize = 48;

/// `db::inhibitedCharacters(false)` @0x1004dee8c. Each one becomes `_`
/// (`replaceInhibitedCharactersWithUnderscores` @0x1004ded7c).
pub const INHIBITED: [char; 10] = ['/', '\\', ':', '?', '*', '"', '<', '>', '|', '~'];

/// Directory used when the track has no artist (or the directory cannot be created).
pub const UNKNOWN_ARTIST: &str = "UnknownArtist";
/// Directory used when the track has no album.
pub const UNKNOWN_ALBUM: &str = "UnknownAlbum";

/// Replaces every inhibited character with `_`, one for one.
pub fn replace_inhibited(s: &str) -> String {
    s.chars()
        .map(|c| if INHIBITED.contains(&c) { '_' } else { c })
        .collect()
}

/// `juce::String::trim`: whitespace off both ends. JUCE tests `iswspace`, which in the C locale
/// rekordbox runs with is ASCII whitespace only.
fn juce_trim(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c'))
}

/// Canonical composition (`convertToPrecomposedUnicode`).
pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// The first `n` code points of `s`.
fn take_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Length in code points (juce `String::length`).
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// One directory level (artist or album), `docs/re/export.md` § 1.3 steps 1–2:
/// 1. an empty name gives `fallback`; otherwise trim, then inhibited characters → `_`;
/// 2. a trailing `.` becomes `_` (FAT drops trailing dots);
/// 3. NFC;
/// 4. at most 48 code points, trimmed again, trailing dots stripped.
///
/// A result that ends up empty (a name of only spaces or dots) also gives `fallback`: rekordbox
/// would try to create a directory with an empty name, fail and use the fallback (step 5).
pub fn directory_component(name: &str, fallback: &str) -> String {
    if name.is_empty() {
        return fallback.to_owned();
    }
    let mut s = replace_inhibited(juce_trim(name));
    if s.ends_with('.') {
        s.pop();
        s.push('_');
    }
    let s = nfc(&s);
    let cut = juce_trim(take_chars(&s, MAX_COMPONENT)).trim_end_matches('.');
    if cut.is_empty() {
        fallback.to_owned()
    } else {
        cut.to_owned()
    }
}

/// Splits a file name at its last `.` (juce `File::getFileExtension`): `("name", ".ext")`, or
/// `(name, "")` without a dot.
pub fn split_extension(file_name: &str) -> (&str, &str) {
    match file_name.rfind('.') {
        Some(i) => (&file_name[..i], &file_name[i..]),
        None => (file_name, ""),
    }
}

/// The device file name for a source file name (§ 1.3 step 4): the name without extension with
/// inhibited characters replaced, plus the original extension, in NFC; longer than 48 code
/// points → the first `48 − len(ext)` code points followed by the extension. Neither the trim
/// nor the trailing-dot rule applies to file names.
pub fn file_component(source_file_name: &str) -> String {
    let (stem, ext) = split_extension(source_file_name);
    let full = nfc(&format!("{}{ext}", replace_inhibited(stem)));
    if char_len(&full) <= MAX_COMPONENT {
        return full;
    }
    let ext = nfc(ext);
    let keep = MAX_COMPONENT.saturating_sub(char_len(&ext));
    format!("{}{ext}", take_chars(&full, keep))
}

/// The `n`-th clash candidate (`n` ≥ 1) for a file name that already exists (§ 1.4,
/// `calc_device_file_path` @0x1009b6e0c): `base-n.ext` while that fits into 48 code points,
/// otherwise the base is cut so that it does.
pub fn clash_candidate(file_name: &str, n: u32) -> String {
    let (base, ext) = split_extension(file_name);
    let suffix = format!("-{n}");
    if char_len(&suffix) + char_len(file_name) <= MAX_COMPONENT {
        format!("{base}{suffix}{ext}")
    } else {
        let keep = MAX_COMPONENT.saturating_sub(char_len(&suffix) + char_len(ext));
        format!("{}{suffix}{ext}", take_chars(base, keep))
    }
}

/// The device directory `/Contents/<artist>/<album>` for a track.
pub fn contents_dir(artist: &str, album: &str) -> String {
    format!(
        "/Contents/{}/{}",
        directory_component(artist, UNKNOWN_ARTIST),
        directory_component(album, UNKNOWN_ALBUM)
    )
}

/// The device path `/Contents/<artist>/<album>/<file>` before clash handling.
pub fn device_path_candidate(artist: &str, album: &str, source_file_name: &str) -> String {
    format!(
        "{}/{}",
        contents_dir(artist, album),
        file_component(source_file_name)
    )
}

/// Key used to decide whether two device paths name the same file on the stick: FAT, exFAT and
/// HFS+ compare names case-insensitively and macOS hands names back in NFD or NFC, so paths are
/// compared in NFC lower case.
pub fn path_key(device_path: &str) -> String {
    nfc(device_path).to_lowercase()
}

/// `/PIONEER` or `/.PIONEER` (HFS volumes, `docs/re/export.md` § 6.2).
pub fn pioneer_dir(hidden: bool) -> &'static str {
    if hidden { ".PIONEER" } else { "PIONEER" }
}

/// Device path of ANLZ set `number` for a track (§ 1.6):
/// `/PIONEER/USBANLZ/P%03X/%08X/ANLZ%04X.DAT`.
pub fn anlz_dat_path(device_path: &str, number: u32, hidden: bool) -> String {
    format!(
        "/{}/ANLZ{number:04X}.DAT",
        mb_anlz::usb::usb_anlz_dir(device_path, hidden)
    )
}

/// `/PIONEER/USBANLZ/P%03X/%08X` of a track (no trailing slash).
pub fn anlz_folder(device_path: &str, hidden: bool) -> String {
    format!("/{}", mb_anlz::usb::usb_anlz_dir(device_path, hidden))
}

/// The number of an `ANLZxxxx.*` file name, if it is one.
pub fn anlz_number(file_name: &str) -> Option<u32> {
    let rest = file_name.strip_prefix("ANLZ")?;
    let (hex, _) = rest.split_once('.')?;
    if hex.is_empty() || hex.len() > 8 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// Folder of an artwork ID: `id / 20 + 1` as `%05d` (§ 3).
pub fn artwork_folder(id: u32) -> String {
    format!("{:05}", id / 20 + 1)
}

/// Artwork row path: `/PIONEER/Artwork/%05d/<prefix><id>.jpg`; prefix `a` for export.pdb
/// (`dsql_calc_next_device_image_info` @0x1019a8c38), `b` for exportLibrary.db
/// (`calc_next_device_image_path` @0x10070a018).
pub fn artwork_path(prefix: char, id: u32, hidden: bool) -> String {
    format!(
        "/{}/Artwork/{}/{prefix}{id}.jpg",
        pioneer_dir(hidden),
        artwork_folder(id)
    )
}

/// The `_m` (240×240) companion of an artwork path: `…/a12.jpg` → `…/a12_m.jpg`.
pub fn artwork_variant(path: &str, suffix: &str) -> String {
    let (stem, ext) = split_extension(path);
    format!("{stem}{suffix}{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inhibited_characters_and_fixture_examples() {
        assert_eq!(
            directory_component("Who Knows?", UNKNOWN_ARTIST),
            "Who Knows_"
        );
        assert_eq!(
            directory_component("Alert / Fog", UNKNOWN_ARTIST),
            "Alert _ Fog"
        );
        assert_eq!(
            directory_component("Culture: Psy", UNKNOWN_ARTIST),
            "Culture_ Psy"
        );
        assert_eq!(
            directory_component("\"der turnbeutel\"", UNKNOWN_ARTIST),
            "_der turnbeutel_"
        );
        assert_eq!(
            directory_component("a~b|c<d>e*f\\g", UNKNOWN_ARTIST),
            "a_b_c_d_e_f_g"
        );
    }

    #[test]
    fn trailing_dots() {
        assert_eq!(directory_component("Bedran.", UNKNOWN_ARTIST), "Bedran_");
        assert_eq!(directory_component("P.R.O.G.", UNKNOWN_ARTIST), "P.R.O.G_");
        // File names keep their dots.
        assert_eq!(
            file_component("Bedran. - Every Time (Original Mix).aiff"),
            "Bedran. - Every Time (Original Mix).aiff"
        );
    }

    #[test]
    fn empty_names_fall_back() {
        assert_eq!(directory_component("", UNKNOWN_ARTIST), "UnknownArtist");
        assert_eq!(directory_component("", UNKNOWN_ALBUM), "UnknownAlbum");
        assert_eq!(directory_component("   ", UNKNOWN_ALBUM), "UnknownAlbum");
        assert_eq!(
            device_path_candidate("", "", "click.mp3"),
            "/Contents/UnknownArtist/UnknownAlbum/click.mp3"
        );
    }

    #[test]
    fn directory_cut_trims_and_strips_dots() {
        // 47 characters, then " ." → cut at 48 gives "<47> " → trimmed.
        let name = format!("{} .xyz", "a".repeat(47));
        assert_eq!(directory_component(&name, UNKNOWN_ALBUM), "a".repeat(47));
        let name = format!("{}...more", "b".repeat(46));
        assert_eq!(directory_component(&name, UNKNOWN_ALBUM), "b".repeat(46));
        let long = "x".repeat(60);
        assert_eq!(char_len(&directory_component(&long, UNKNOWN_ALBUM)), 48);
    }

    #[test]
    fn file_name_cut_keeps_the_extension() {
        let f = file_component("Ajja, Alpha Portal - The Other Side (Original Mix).aiff");
        assert_eq!(f, "Ajja, Alpha Portal - The Other Side (Origin.aiff");
        assert_eq!(char_len(&f), 48);
        assert_eq!(file_component("a?b.mp3"), "a_b.mp3");
        assert_eq!(file_component("noext"), "noext");
    }

    #[test]
    fn nfc_and_code_points() {
        // "é" as e + combining acute (NFD) becomes one code point.
        let nfd = "Cafe\u{301}";
        assert_eq!(directory_component(nfd, UNKNOWN_ARTIST), "Caf\u{e9}");
        let long = "\u{e9}".repeat(50);
        assert_eq!(char_len(&directory_component(&long, UNKNOWN_ARTIST)), 48);
    }

    #[test]
    fn clash_candidates() {
        assert_eq!(
            clash_candidate("Distinctside - Inside (Original Mix).aiff", 1),
            "Distinctside - Inside (Original Mix)-1.aiff"
        );
        let full = format!("{}.mp3", "z".repeat(44)); // 48
        let c = clash_candidate(&full, 1);
        assert_eq!(c, format!("{}-1.mp3", "z".repeat(42)));
        assert_eq!(char_len(&c), 48);
        let c = clash_candidate(&full, 12);
        assert_eq!(c, format!("{}-12.mp3", "z".repeat(41)));
    }

    #[test]
    fn anlz_and_artwork_paths() {
        assert_eq!(anlz_number("ANLZ0000.DAT"), Some(0));
        assert_eq!(anlz_number("ANLZ001F.EXT"), Some(0x1f));
        assert_eq!(anlz_number("other.DAT"), None);
        assert_eq!(artwork_path('a', 1, false), "/PIONEER/Artwork/00001/a1.jpg");
        assert_eq!(
            artwork_path('b', 20, false),
            "/PIONEER/Artwork/00002/b20.jpg"
        );
        assert_eq!(
            artwork_path('a', 702, false),
            "/PIONEER/Artwork/00036/a702.jpg"
        );
        assert_eq!(
            artwork_variant("/PIONEER/Artwork/00001/a1.jpg", "_m"),
            "/PIONEER/Artwork/00001/a1_m.jpg"
        );
        let p = anlz_dat_path("/Contents/a/b.mp3", 0, false);
        assert!(p.starts_with("/PIONEER/USBANLZ/P") && p.ends_with("/ANLZ0000.DAT"));
    }

    #[test]
    fn path_keys_ignore_case_and_normalisation() {
        assert_eq!(
            path_key("/Contents/Cafe\u{301}/X.mp3"),
            path_key("/contents/CAF\u{c9}/x.MP3")
        );
    }
}
