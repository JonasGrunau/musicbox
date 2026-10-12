//! Musical key notation. rekordbox stores classic names (`djmdKey.ScaleName`, e.g. `Am`, `F#`)
//! and renders either classic or Camelot-style alphanumeric (`8A`) depending on a preference.
//! The binary's key-name pool @0x104f7e346 lists per Camelot slot: alphanumeric, zero-padded
//! alphanumeric, classic, alternate classic, Open Key (e.g. `1A 01A Abm G#m 6m`). Minor names
//! below are verified against it; major primaries share pooled strings and still need the
//! pointer table (TODO docs/re/keys.md).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyNotation {
    /// The stored name as is (rekordbox's default: preference `ShowOriginalKey` on).
    Original,
    Classic,
    Alphanumeric,
}

impl KeyNotation {
    /// The notation rekordbox displays for the preferences `ShowOriginalKey` (default on) and
    /// `KeyStringSetting` (default 1), as `MusicKey::getKeyNotationSetting` @0x1003662f4 decides:
    /// original names when `ShowOriginalKey` is on, else 2 = alphanumeric
    /// (`isKeyDisplaySettingAlphanumeric` @0x1003663ac) and anything else classic.
    pub fn from_preferences(show_original_key: Option<bool>, key_string_setting: Option<i64>) -> Self {
        if show_original_key.unwrap_or(true) {
            KeyNotation::Original
        } else if key_string_setting.unwrap_or(1) == 2 {
            KeyNotation::Alphanumeric
        } else {
            KeyNotation::Classic
        }
    }
}

/// (classic, alphanumeric) for the 24 keys, minor then major per Camelot number.
const KEYS: [(&str, &str); 24] = [
    ("Abm", "1A"), ("B", "1B"),
    ("Ebm", "2A"), ("F#", "2B"),
    ("Bbm", "3A"), ("Db", "3B"),
    ("Fm", "4A"), ("Ab", "4B"),
    ("Cm", "5A"), ("Eb", "5B"),
    ("Gm", "6A"), ("Bb", "6B"),
    ("Dm", "7A"), ("F", "7B"),
    ("Am", "8A"), ("C", "8B"),
    ("Em", "9A"), ("G", "9B"),
    ("Bm", "10A"), ("D", "10B"),
    ("F#m", "11A"), ("A", "11B"),
    ("Dbm", "12A"), ("E", "12B"),
];

/// Long forms (`Gmin`, `D#maj`, as stored in device key tables) to the short classic form.
fn short_form(name: &str) -> std::borrow::Cow<'_, str> {
    if let Some(root) = name.strip_suffix("min") {
        format!("{root}m").into()
    } else if let Some(root) = name.strip_suffix("maj") {
        root.into()
    } else {
        name.into()
    }
}

/// Enharmonic spellings mapped onto the names used in [`KEYS`].
fn canonical(name: &str) -> &str {
    match name {
        "G#m" => "Abm",
        "D#m" => "Ebm",
        "A#m" => "Bbm",
        "C#m" => "Dbm",
        "Gb" => "F#",
        "C#" => "Db",
        "G#" => "Ab",
        "D#" => "Eb",
        "A#" => "Bb",
        "Gbm" => "F#m",
        "Cb" => "B",
        other => other,
    }
}

/// Renders a stored key name in the requested notation; unknown names pass through unchanged.
pub fn display(stored: &str, notation: KeyNotation) -> String {
    let short = short_form(stored.trim());
    let name = canonical(&short);
    match notation {
        KeyNotation::Original => stored.to_owned(),
        KeyNotation::Classic => name.to_owned(),
        KeyNotation::Alphanumeric => KEYS
            .iter()
            .find(|(classic, _)| *classic == name)
            .map_or_else(|| stored.to_owned(), |(_, alnum)| (*alnum).to_owned()),
    }
}

/// Sort position on the Camelot wheel (1A, 1B, 2A, …); unknown keys sort last.
pub fn wheel_index(stored: &str) -> usize {
    let short = short_form(stored.trim());
    let name = canonical(&short);
    KEYS.iter().position(|(classic, _)| *classic == name).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notation_from_preferences() {
        use KeyNotation::*;
        assert_eq!(KeyNotation::from_preferences(None, None), Original);
        assert_eq!(KeyNotation::from_preferences(Some(true), Some(2)), Original);
        assert_eq!(KeyNotation::from_preferences(Some(false), Some(2)), Alphanumeric);
        assert_eq!(KeyNotation::from_preferences(Some(false), Some(1)), Classic);
        assert_eq!(KeyNotation::from_preferences(Some(false), None), Classic);
        assert_eq!(display(" Am ", Original), " Am ");
    }

    #[test]
    fn alphanumeric() {
        assert_eq!(display("Am", KeyNotation::Alphanumeric), "8A");
        assert_eq!(display("G#m", KeyNotation::Alphanumeric), "1A");
        assert_eq!(display("C", KeyNotation::Alphanumeric), "8B");
        assert_eq!(display("weird", KeyNotation::Alphanumeric), "weird");
        assert_eq!(display("Gmin", KeyNotation::Alphanumeric), "6A");
        assert_eq!(display("D#maj", KeyNotation::Alphanumeric), "5B");
        assert_eq!(display("Gmin", KeyNotation::Classic), "Gm");
    }
}
