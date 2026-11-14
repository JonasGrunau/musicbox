//! Intelligent playlists ("smart lists"): the rule XML rekordbox keeps in
//! `djmdPlaylist.SmartList` for `Attribute` 4 rows, and its evaluation over the collection.
//! Spec: `docs/re/smartlist.md`.
//!
//! - [`SmartList::parse`] reads the XML the way `db::getSmartlistNode` @0x1020b4c88 and
//!   `db::getSmartlistCondition` @0x1020b57fc do (one `NODE`, its `CONDITION` children; unknown
//!   properties and invalid operators are dropped).
//! - [`SmartList::to_xml`] writes it as the edit dialog does: `juce::XmlElement::toString` with
//!   the default header, CRLF, two-space indentation, attribute wrapping after 60 bytes and
//!   JUCE's escaping (`smartlist.md` § Serialisation).
//! - [`SmartList::evaluate`] replays `db::getSmartlistContentData` @0x1020b4684 and
//!   `db::operate` @0x1020b5010: every collection track in collection order, all (1) or any (2)
//!   of the conditions. Nothing is stored; rekordbox evaluates on every read.

use crate::{DbId, Track};
use std::collections::HashMap;

/// `LogicalOperator` 1: a track must match every condition.
pub const MATCH_ALL: i32 = 1;
/// `LogicalOperator` 2: one matching condition is enough.
pub const MATCH_ANY: i32 = 2;

/// Operator codes (`xml_operator` @0x1052ea088: index = code).
pub mod op {
    pub const IS: i32 = 1;
    pub const IS_NOT: i32 = 2;
    pub const GREATER: i32 = 3;
    pub const LESS: i32 = 4;
    pub const IN_RANGE: i32 = 5;
    pub const IN_LAST: i32 = 6;
    pub const NOT_IN_LAST: i32 = 7;
    pub const CONTAINS: i32 = 8;
    pub const NOT_CONTAINS: i32 = 9;
    pub const STARTS_WITH: i32 = 10;
    pub const ENDS_WITH: i32 = 11;
}

/// The operator labels of rekordbox's dialog, by code.
pub fn operator_label(code: i32) -> &'static str {
    match code {
        op::IS => "is",
        op::IS_NOT => "is not",
        op::GREATER => "is greater than",
        op::LESS => "is less than",
        op::IN_RANGE => "is in the range",
        op::IN_LAST => "is in the last",
        op::NOT_IN_LAST => "is not in the last",
        op::CONTAINS => "contains",
        op::NOT_CONTAINS => "does not contain",
        op::STARTS_WITH => "starts with",
        op::ENDS_WITH => "ends with",
        _ => "?",
    }
}

/// How a property is compared (`ConditionComp::getConditionType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Text,
    Number,
    Date,
    MyTag,
    Color,
}

impl ValueKind {
    /// The operators the dialog offers (`ConditionComp::updateComboBoxCondition` @0x10216a0e4).
    pub fn operators(self) -> &'static [i32] {
        use op::*;
        match self {
            Self::Text => &[IS, IS_NOT, CONTAINS, NOT_CONTAINS, STARTS_WITH, ENDS_WITH],
            Self::Number => &[IS, IS_NOT, GREATER, LESS, IN_RANGE],
            Self::Date => &[IS, IS_NOT, GREATER, LESS, IN_LAST, NOT_IN_LAST, IN_RANGE],
            Self::MyTag => &[CONTAINS, NOT_CONTAINS],
            Self::Color => &[IS, IS_NOT],
        }
    }
}

/// A condition property: its `PropertyName`, the dialog label and its kind, in the dialog's
/// order (`xml_property_name1..5` @0x1052e9f90, labels `property_name1..5` @0x1053afe20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Property {
    pub xml: &'static str,
    pub label: &'static str,
    pub kind: ValueKind,
}

pub const PROPERTIES: &[Property] = &[
    Property { xml: "name", label: "Track Title", kind: ValueKind::Text },
    Property { xml: "artist", label: "Artist", kind: ValueKind::Text },
    Property { xml: "album", label: "Album", kind: ValueKind::Text },
    Property { xml: "genre", label: "Genre", kind: ValueKind::Text },
    Property { xml: "albumArtist", label: "Album artist", kind: ValueKind::Text },
    Property { xml: "comments", label: "Comments", kind: ValueKind::Text },
    Property { xml: "producer", label: "Composer", kind: ValueKind::Text },
    Property { xml: "mixName", label: "Mix name", kind: ValueKind::Text },
    Property { xml: "remixedBy", label: "Remixer", kind: ValueKind::Text },
    Property { xml: "label", label: "Label", kind: ValueKind::Text },
    Property { xml: "key", label: "Key", kind: ValueKind::Text },
    Property { xml: "originalArtist", label: "Original artist", kind: ValueKind::Text },
    Property { xml: "fileName", label: "File name", kind: ValueKind::Text },
    Property { xml: "bpm", label: "BPM", kind: ValueKind::Number },
    Property { xml: "duration", label: "Time", kind: ValueKind::Number },
    Property { xml: "year", label: "Year", kind: ValueKind::Number },
    Property { xml: "counter", label: "DJ play count", kind: ValueKind::Number },
    Property { xml: "rating", label: "Rating", kind: ValueKind::Number },
    Property { xml: "dateReleased", label: "Release Date", kind: ValueKind::Date },
    Property { xml: "stockDate", label: "Date Added", kind: ValueKind::Date },
    Property { xml: "dateCreated", label: "Date Created", kind: ValueKind::Date },
    Property { xml: "myTag", label: "My Tag", kind: ValueKind::MyTag },
    Property { xml: "grouping", label: "Color", kind: ValueKind::Color },
];

/// The property with this `PropertyName` (compared case-insensitively, as the parser does).
pub fn property(xml_name: &str) -> Option<&'static Property> {
    PROPERTIES.iter().find(|p| p.xml.eq_ignore_ascii_case(xml_name))
}

/// One `CONDITION`. The values are kept as the strings stored in the XML so that a parse/write
/// round trip is exact; [`Condition::new`] builds them as the dialog does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
    pub property: String,
    pub operator: i32,
    /// `ValueUnit`: `day` / `month` for "in the last"; `"0"` for BPM; otherwise empty.
    pub unit: String,
    pub left: String,
    pub right: String,
}

impl Condition {
    /// A condition with values in the dialog's format (`SmartPlaylist::Condition::createCondition`
    /// @0x102164dd8): BPM values are entered as decimals (`128.5`) and stored ×100 (`12850`),
    /// empty ones as `0`; every other value is stored as given (text, an integer, a date
    /// `YYYY-MM-DD`, a My Tag ID, a colour ID 1..8, a rating 0..5, a time in seconds).
    pub fn new(property: &str, operator: i32, unit: &str, left: &str, right: &str) -> Self {
        if property == "bpm" {
            Self {
                property: property.to_owned(),
                operator,
                unit: bpm_x100(unit).to_string(),
                left: bpm_x100(left).to_string(),
                right: bpm_x100(right).to_string(),
            }
        } else {
            Self {
                property: property.to_owned(),
                operator,
                unit: unit.to_owned(),
                left: left.to_owned(),
                right: right.to_owned(),
            }
        }
    }

    pub fn kind(&self) -> Option<ValueKind> {
        property(&self.property).map(|p| p.kind)
    }
}

/// `createCondition`'s BPM conversion: `"128"` → 12800; with a dot, the integer part ×100 plus the
/// first decimal ×10 plus the second (`"128.5"` → 12850, `"128.05"` → 12805).
fn bpm_x100(s: &str) -> i32 {
    match s.find('.') {
        None => juce_int(s).wrapping_mul(100),
        Some(i) => {
            let int = juce_int(&s[..i]);
            let d1 = juce_int(&s[i + 1..]);
            let d2 = juce_int(s.get(i + 2..).unwrap_or(""));
            int.wrapping_mul(100).wrapping_add(d1.wrapping_mul(10)).wrapping_add(d2)
        }
    }
}

/// `juce::String::getIntValue`: optional leading whitespace and sign, then digits; 0 otherwise.
pub fn juce_int(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut v: i32 = 0;
    for b in digits.bytes() {
        if !b.is_ascii_digit() {
            break;
        }
        v = v.wrapping_mul(10).wrapping_add(i32::from(b - b'0'));
    }
    if neg { v.wrapping_neg() } else { v }
}

/// One smart list: the `NODE` and its conditions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartList {
    /// `Id`: the playlist's ID as a signed 32-bit value (`juce::XmlElement::setAttribute(int)`).
    pub id: i32,
    /// [`MATCH_ALL`] or [`MATCH_ANY`].
    pub logical_operator: i32,
    /// `AutomaticUpdate`; rekordbox's dialog always writes 0 (`Node::createNode(…, false)`), and
    /// the evaluation does not read it.
    pub automatic_update: bool,
    pub conditions: Vec<Condition>,
}

impl SmartList {
    pub fn new(playlist_id: &str, logical_operator: i32, conditions: Vec<Condition>) -> Self {
        Self {
            id: playlist_id.parse::<u32>().map_or(0, |v| v as i32),
            logical_operator,
            automatic_update: false,
            conditions,
        }
    }

    /// Parses the stored XML. `None` when there is no `NODE` root. Conditions rekordbox would
    /// drop (unknown property, operator outside 1..11, or 8..11 with an empty `ValueLeft` for a
    /// text or My Tag property, or 8..11 for any other property) are dropped here too, and a
    /// `LogicalOperator` other than 1 or 2 becomes 1 when there are conditions.
    pub fn parse(xml: &str) -> Option<Self> {
        let root = xml::parse(xml)?;
        if !root.name.eq_ignore_ascii_case("NODE") {
            return None;
        }
        let mut conditions = Vec::new();
        for c in &root.children {
            if !c.name.eq_ignore_ascii_case("CONDITION") {
                continue;
            }
            let cond = Condition {
                property: c.attr("PropertyName").to_owned(),
                operator: juce_int(c.attr("Operator")),
                unit: c.attr("ValueUnit").to_owned(),
                left: c.attr("ValueLeft").to_owned(),
                right: c.attr("ValueRight").to_owned(),
            };
            if valid(&cond) {
                conditions.push(cond);
            }
        }
        let mut logical_operator = juce_int(root.attr("LogicalOperator"));
        if !conditions.is_empty() && logical_operator != MATCH_ALL && logical_operator != MATCH_ANY {
            logical_operator = MATCH_ALL;
        }
        Some(Self {
            id: juce_int(root.attr("Id")),
            logical_operator,
            automatic_update: juce_int(root.attr("AutomaticUpdate")) != 0,
            conditions,
        })
    }

    /// The text rekordbox stores (`SmartPlaylistComp::buttonClicked` @0x102165ff0 →
    /// `juce::XmlElement::toString` with encoding `UTF-8`).
    pub fn to_xml(&self) -> String {
        let mut node = xml::Element::new("NODE");
        node.set("Id", self.id.to_string());
        node.set("LogicalOperator", self.logical_operator.to_string());
        node.set("AutomaticUpdate", if self.automatic_update { "1" } else { "0" });
        for c in &self.conditions {
            let mut e = xml::Element::new("CONDITION");
            e.set("PropertyName", &c.property);
            e.set("Operator", c.operator.to_string());
            e.set("ValueUnit", &c.unit);
            e.set("ValueLeft", &c.left);
            e.set("ValueRight", &c.right);
            node.children.push(e);
        }
        node.to_document()
    }

    /// The matching tracks, in collection order. `my_tags` maps a track to its My Tag IDs.
    pub fn evaluate<'a>(
        &self,
        tracks: &'a [Track],
        my_tags: &HashMap<DbId, Vec<i64>>,
        clock: Clock,
    ) -> Vec<&'a DbId> {
        let conds: Vec<Compiled> = self.conditions.iter().filter_map(|c| compile(c, clock)).collect();
        if conds.is_empty() {
            // `getSmartlistNode` fails without conditions: the list is empty.
            return Vec::new();
        }
        let none: Vec<i64> = Vec::new();
        tracks
            .iter()
            .filter(|t| {
                let tags = my_tags.get(&t.id).unwrap_or(&none);
                if self.logical_operator == MATCH_ANY {
                    conds.iter().any(|c| c.matches(t, tags))
                } else {
                    conds.iter().all(|c| c.matches(t, tags))
                }
            })
            .map(|t| &t.id)
            .collect()
    }
}

/// `getSmartlistCondition`'s final check.
fn valid(c: &Condition) -> bool {
    let Some(p) = property(&c.property) else {
        return false;
    };
    match c.operator {
        1..=7 => true,
        8..=11 => matches!(p.kind, ValueKind::Text | ValueKind::MyTag) && !c.left.is_empty(),
        _ => false,
    }
}

// ---- evaluation ----------------------------------------------------------------------------

/// The time the evaluation runs at: Unix seconds and the local UTC offset in seconds.
/// "In the last N days/months" depends on it (`pastDayToDay` @0x1020b6600,
/// `pastMonthToDay` @0x1020b6524).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub unix: i64,
    pub utc_offset: i64,
}

impl Clock {
    /// Now in UTC (no local offset); callers with a time-zone source pass their own.
    pub fn utc_now() -> Self {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        Self { unix, utc_offset: 0 }
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date; months outside 1..12 roll over like
/// `mktime` does, days count on from the first of the month.
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let m0 = month - 1;
    let y = year + m0.div_euclid(12);
    let m = m0.rem_euclid(12) + 1;
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468 + (day - 1)
}

/// (year, month, day) of a day number.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `db::dateToDay` @0x1020b568c: a 10-character `YYYY-MM-DD` (digits read by position, the
/// separators are not checked) → its day number; 0 for anything else or for dates whose local
/// midnight is not after the epoch.
pub fn date_to_day(s: &str, clock: Clock) -> i64 {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() != 10 {
        return 0;
    }
    let d = |i: usize| chars[i] as i64 - '0' as i64;
    let year = d(0) * 1000 + d(1) * 100 + d(2) * 10 + d(3);
    let month = d(5) * 10 + d(6);
    let day = d(8) * 10 + d(9);
    let days = days_from_civil(year, month, day);
    if days * 86_400 - clock.utc_offset <= 0 {
        return 0;
    }
    days
}

/// `pastDayToDay`: today minus `n - 1` days.
fn past_days(n: i64, clock: Clock) -> i64 {
    if clock.unix <= 0 {
        return 0;
    }
    (clock.unix + clock.utc_offset).div_euclid(86_400) - (n - 1).max(0)
}

/// `pastMonthToDay`: the last day of the month `n` months back (day 0 of the month `n - 1`
/// back), taken at the current local time of day and converted without the offset.
fn past_months(n: i64, clock: Clock) -> i64 {
    if clock.unix <= 0 {
        return 0;
    }
    let local = clock.unix + clock.utc_offset;
    let (y, m, _) = civil_from_days(local.div_euclid(86_400));
    let seconds_of_day = local.rem_euclid(86_400);
    let day = days_from_civil(y, m - (n - 1).max(0), 1) - 1;
    let t = day * 86_400 + seconds_of_day - clock.utc_offset;
    if t <= 0 {
        return 0;
    }
    t / 86_400
}

/// A condition with its values decoded once, as `getSmartlistCondition` stores them.
struct Compiled {
    property: &'static str,
    kind: ValueKind,
    operator: i32,
    text: String,
    left: i64,
    right: i64,
}

fn compile(c: &Condition, clock: Clock) -> Option<Compiled> {
    if !valid(c) {
        return None;
    }
    let p = property(&c.property)?;
    let mut out = Compiled {
        property: p.xml,
        kind: p.kind,
        operator: c.operator,
        text: String::new(),
        left: 0,
        right: 0,
    };
    match p.kind {
        ValueKind::Text => out.text = fold(&c.left),
        ValueKind::MyTag => {
            out.text = c.left.clone();
            out.left = i64::from(juce_int(&c.left));
        }
        ValueKind::Color => out.left = i64::from(juce_int(&c.left)),
        ValueKind::Number => {
            out.left = i64::from(juce_int(&c.left));
            out.right = i64::from(juce_int(&c.right));
            if p.xml == "rating" {
                // Old lists stored 0..255; values above 50 are divided by 51.
                if out.left > 50 {
                    out.left /= 51;
                }
                if out.right > 50 {
                    out.right /= 51;
                }
            }
        }
        ValueKind::Date => {
            if c.operator == op::IN_LAST || c.operator == op::NOT_IN_LAST {
                let n = i64::from(juce_int(&c.left));
                out.left = if c.unit.eq_ignore_ascii_case("month") {
                    past_months(n, clock)
                } else {
                    past_days(n, clock)
                };
            } else {
                out.left = date_to_day(&c.left, clock);
                out.right = date_to_day(&c.right, clock);
            }
        }
    }
    Some(out)
}

impl Compiled {
    fn matches(&self, t: &Track, tags: &[i64]) -> bool {
        match self.kind {
            ValueKind::Text => text_op(text_field(t, self.property), self),
            ValueKind::MyTag => match self.operator {
                op::CONTAINS => tags.contains(&self.left),
                op::NOT_CONTAINS => !tags.contains(&self.left),
                _ => false,
            },
            ValueKind::Color => {
                let v = i64::from(t.color.unwrap_or(0));
                number_op(v, self)
            }
            ValueKind::Number => {
                let v = match self.property {
                    // RowDataTrack +0x3a8 (int), +0x3d8 / +0x37a / +0x3ac (u16), +0x378 (char).
                    "bpm" => i64::from(t.bpm_x100.unwrap_or(0) as i32),
                    "duration" => i64::from(t.length_seconds.unwrap_or(0) as u16),
                    "year" => i64::from(t.release_year.unwrap_or(0) as u16),
                    "counter" => i64::from(t.play_count as u16),
                    _ => i64::from(t.rating as i8),
                };
                if v < 0 {
                    return false;
                }
                number_op(v, self)
            }
            ValueKind::Date => {
                let s = match self.property {
                    "stockDate" => &t.date_added,
                    "dateReleased" => &t.release_date,
                    _ => &t.date_created,
                };
                // The clock only matters for the condition values; a track date needs none.
                let day = date_to_day(s, Clock { unix: 1, utc_offset: 0 });
                if day < 1 {
                    return false;
                }
                match self.operator {
                    op::IS => day == self.left,
                    op::IS_NOT => day != self.left,
                    op::GREATER => day > self.left,
                    op::LESS | op::NOT_IN_LAST => day < self.left,
                    op::IN_RANGE => self.left <= day && day <= self.right,
                    op::IN_LAST => day >= self.left,
                    _ => false,
                }
            }
        }
    }
}

fn number_op(v: i64, c: &Compiled) -> bool {
    match c.operator {
        op::IS => v == c.left,
        op::IS_NOT => v != c.left,
        op::GREATER => v > c.left,
        op::LESS => v < c.left,
        op::IN_RANGE => c.left <= v && v <= c.right,
        _ => false,
    }
}

/// The RowDataTrack string each text property reads (`db::operate`).
fn text_field<'a>(t: &'a Track, property: &str) -> &'a str {
    match property {
        "name" => &t.title,
        "fileName" => &t.file_name,
        "comments" => &t.comment,
        "mixName" => &t.mix_name,
        "artist" => &t.artist,
        "remixedBy" => &t.remixer,
        "originalArtist" => &t.original_artist,
        "producer" => &t.composer,
        "album" => &t.album,
        "genre" => &t.genre,
        "label" => &t.label,
        "key" => &t.key,
        "albumArtist" => &t.album_artist,
        _ => "",
    }
}

/// `db::operateString` @0x1020b550c.
fn text_op(field: &str, c: &Compiled) -> bool {
    let value_empty = c.text.is_empty();
    if field.is_empty() {
        return match c.operator {
            op::IS_NOT => !value_empty,
            op::IS => value_empty,
            _ => false,
        };
    }
    if value_empty {
        return c.operator == op::IS_NOT;
    }
    let f = fold(field);
    match c.operator {
        op::IS => f == c.text,
        op::IS_NOT => f != c.text,
        op::CONTAINS => f.contains(&c.text),
        op::NOT_CONTAINS => !f.contains(&c.text),
        op::STARTS_WITH => f.starts_with(&c.text),
        op::ENDS_WITH => f.ends_with(&c.text),
        _ => false,
    }
}

/// rekordbox compares with an ICU `StringSearch` whose collator has primary strength
/// (`db::CollationRule` @0x1020b4e44: US locale, `setStrength(PRIMARY)`): case, accents and
/// width do not count. Approximation: NFKD, combining marks removed, lower case, `ß` → `ss`.
pub fn fold(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    use unicode_normalization::char::is_combining_mark;
    // ASCII has no decompositions or combining marks: only the case changes. Most tags are
    // ASCII, and a smart list folds a field of every track.
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.nfkd().filter(|c| !is_combining_mark(*c)) {
        if c == 'ß' || c == 'ẞ' {
            out.push_str("ss");
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The tracks of every smart list in `snapshot`, keyed by playlist ID.
pub fn evaluate_all(snapshot: &crate::LibrarySnapshot, clock: Clock) -> HashMap<DbId, Vec<DbId>> {
    let tags = track_tags(snapshot);
    snapshot
        .smart_lists
        .iter()
        .map(|(id, xml)| {
            let ids = SmartList::parse(xml)
                .map(|l| l.evaluate(&snapshot.tracks, &tags, clock).into_iter().cloned().collect())
                .unwrap_or_default();
            (id.clone(), ids)
        })
        .collect()
}

/// Track → My Tag IDs, from the snapshot's My Tag links.
pub fn track_tags(snapshot: &crate::LibrarySnapshot) -> HashMap<DbId, Vec<i64>> {
    let mut out: HashMap<DbId, Vec<i64>> = HashMap::new();
    for (tag, tracks) in &snapshot.my_tag_entries {
        let Ok(tag_id) = tag.as_str().parse::<i64>() else { continue };
        for t in tracks {
            out.entry(t.clone()).or_default().push(tag_id);
        }
    }
    out
}

// ---- XML ----------------------------------------------------------------------------------

/// A minimal reader and a `juce::XmlElement` writer, enough for the smart list format.
mod xml {
    /// CRLF (`TextFormat::newLineChars` default, @0x1024acb40).
    const NEWLINE: &str = "\r\n";
    /// `TextFormat::lineWrapLength` default 60.
    const LINE_WRAP: usize = 60;

    #[derive(Debug, Default)]
    pub struct Element {
        pub name: String,
        pub attributes: Vec<(String, String)>,
        pub children: Vec<Element>,
    }

    impl Element {
        pub fn new(name: &str) -> Self {
            Self { name: name.to_owned(), ..Self::default() }
        }

        pub fn set(&mut self, name: &str, value: impl Into<String>) {
            self.attributes.push((name.to_owned(), value.into()));
        }

        /// `getStringAttribute`: exact name, "" when missing.
        pub fn attr(&self, name: &str) -> &str {
            self.attributes.iter().find(|(n, _)| n == name).map_or("", |(_, v)| v.as_str())
        }

        /// `juce::XmlElement::writeTo` @0x1024f3480: default header, two newlines, the element
        /// at indent 0, a final newline.
        pub fn to_document(&self) -> String {
            let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
            out.push_str(NEWLINE);
            out.push_str(NEWLINE);
            self.write(&mut out, 0);
            out.push_str(NEWLINE);
            out
        }

        /// `juce::XmlElement::writeElementAsText` @0x1024f2938.
        fn write(&self, out: &mut String, indent: usize) {
            out.extend(std::iter::repeat_n(' ', indent));
            out.push('<');
            out.push_str(&self.name);
            let attr_indent = indent + self.name.chars().count() + 1;
            let mut line_len = 0usize;
            for (name, value) in &self.attributes {
                if line_len > LINE_WRAP {
                    out.push_str(NEWLINE);
                    out.extend(std::iter::repeat_n(' ', attr_indent));
                    line_len = 0;
                }
                let start = out.len();
                out.push(' ');
                out.push_str(name);
                out.push_str("=\"");
                escape(out, value);
                out.push('"');
                line_len += out.len() - start;
            }
            if self.children.is_empty() {
                out.push_str("/>");
                return;
            }
            out.push('>');
            for c in &self.children {
                out.push_str(NEWLINE);
                c.write(out, indent + 2);
            }
            out.push_str(NEWLINE);
            out.extend(std::iter::repeat_n(' ', indent));
            out.push_str("</");
            out.push_str(&self.name);
            out.push('>');
        }
    }

    /// `juce::XmlOutputFunctions::escapeIllegalXmlChars` @0x1024f3014 for attributes: ASCII
    /// characters of the legal table (@0x103882c84) stay; `" & < >` become entities; everything
    /// else (CR and LF included, every non-ASCII character) becomes `&#<decimal code point>;`.
    fn escape(out: &mut String, value: &str) {
        for c in value.chars() {
            match c {
                '"' => out.push_str("&quot;"),
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                c if legal(c) => out.push(c),
                c => {
                    out.push_str("&#");
                    out.push_str(&(c as u32).to_string());
                    out.push(';');
                }
            }
        }
    }

    /// The legal-character table: space, letters, digits and ``!#$%'()*+,-./:;=?@[\]_{|}~``.
    fn legal(c: char) -> bool {
        c.is_ascii_alphanumeric() || " !#$%'()*+,-./:;=?@[\\]_{|}~".contains(c)
    }

    /// Parses the first element of `text` (declaration, comments and whitespace before it are
    /// skipped; text content is ignored).
    pub fn parse(text: &str) -> Option<Element> {
        let mut p = Parser { s: text.strip_prefix('\u{feff}').unwrap_or(text), i: 0 };
        loop {
            p.skip_ws();
            if p.rest().starts_with("<?") {
                p.i += p.rest().find("?>")? + 2;
            } else if p.rest().starts_with("<!--") {
                p.i += p.rest().find("-->")? + 3;
            } else if p.rest().starts_with("<!") {
                p.i += p.rest().find('>')? + 1;
            } else {
                break;
            }
        }
        p.element()
    }

    struct Parser<'a> {
        s: &'a str,
        i: usize,
    }

    impl Parser<'_> {
        fn rest(&self) -> &str {
            &self.s[self.i..]
        }

        fn skip_ws(&mut self) {
            let n = self.rest().len() - self.rest().trim_start().len();
            self.i += n;
        }

        fn name(&mut self) -> String {
            let n = self
                .rest()
                .find(|c: char| c.is_whitespace() || c == '=' || c == '/' || c == '>')
                .unwrap_or(self.rest().len());
            let name = self.rest()[..n].to_owned();
            self.i += n;
            name
        }

        fn element(&mut self) -> Option<Element> {
            if !self.rest().starts_with('<') {
                return None;
            }
            self.i += 1;
            let mut e = Element::new(&self.name());
            if e.name.is_empty() {
                return None;
            }
            loop {
                self.skip_ws();
                let r = self.rest();
                if r.starts_with("/>") {
                    self.i += 2;
                    return Some(e);
                }
                if r.starts_with('>') {
                    self.i += 1;
                    break;
                }
                if r.is_empty() {
                    return None;
                }
                let name = self.name();
                if name.is_empty() {
                    return None;
                }
                self.skip_ws();
                if !self.rest().starts_with('=') {
                    return None;
                }
                self.i += 1;
                self.skip_ws();
                let quote = self.rest().chars().next()?;
                if quote != '"' && quote != '\'' {
                    return None;
                }
                self.i += 1;
                let end = self.rest().find(quote)?;
                let value = unescape(&self.rest()[..end]);
                self.i += end + 1;
                e.attributes.push((name, value));
            }
            // Content: child elements until the closing tag; text and comments are skipped.
            loop {
                let lt = self.rest().find('<')?;
                self.i += lt;
                let r = self.rest();
                if r.starts_with("</") {
                    self.i += r.find('>')? + 1;
                    return Some(e);
                } else if r.starts_with("<!--") {
                    self.i += r.find("-->")? + 3;
                } else if r.starts_with("<?") || r.starts_with("<!") {
                    self.i += r.find('>')? + 1;
                } else {
                    let child = self.element()?;
                    e.children.push(child);
                }
            }
        }
    }

    fn unescape(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(amp) = rest.find('&') {
            out.push_str(&rest[..amp]);
            rest = &rest[amp..];
            let Some(semi) = rest.find(';') else { break };
            let entity = &rest[1..semi];
            let decoded = match entity {
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "lt" => Some('<'),
                "gt" => Some('>'),
                e if e.starts_with("#x") || e.starts_with("#X") => {
                    u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32)
                }
                e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            match decoded {
                Some(c) => {
                    out.push(c);
                    rest = &rest[semi + 1..];
                }
                None => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
        }
        out.push_str(rest);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileType;
    use std::path::PathBuf;

    fn track(id: &str) -> Track {
        Track {
            id: DbId(id.into()),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            genre: String::new(),
            label: String::new(),
            remixer: String::new(),
            composer: String::new(),
            original_artist: String::new(),
            album_artist: String::new(),
            mix_name: String::new(),
            file_name: String::new(),
            key: String::new(),
            color: None,
            bpm_x100: None,
            length_seconds: None,
            track_no: None,
            disc_no: None,
            release_year: None,
            rating: 0,
            comment: String::new(),
            play_count: 0,
            bitrate: None,
            sample_rate: None,
            bit_depth: None,
            file_size: None,
            file_type: FileType::Mp3,
            path: PathBuf::new(),
            date_added: String::new(),
            date_created: String::new(),
            release_date: String::new(),
            analysis_path: None,
            artwork_path: None,
            analysed: 0,
        }
    }

    const CLOCK: Clock = Clock { unix: 1_791_892_800, utc_offset: 7200 }; // 2026-10-13 12:00 UTC

    fn eval(list: &SmartList, tracks: &[Track], tags: &HashMap<DbId, Vec<i64>>) -> Vec<String> {
        list.evaluate(tracks, tags, CLOCK).into_iter().map(|d| d.0.clone()).collect()
    }

    fn one(c: Condition) -> SmartList {
        SmartList { id: 1, logical_operator: MATCH_ALL, automatic_update: false, conditions: vec![c] }
    }

    #[test]
    fn serialises_like_juce() {
        let list = SmartList {
            id: 123456,
            logical_operator: MATCH_ALL,
            automatic_update: false,
            conditions: vec![
                Condition::new("artist", op::CONTAINS, "", "Café & \"Co\"", ""),
                Condition::new("bpm", op::IN_RANGE, "", "120", "128.5"),
            ],
        };
        let xml = list.to_xml();
        let expected = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n",
            "<NODE Id=\"123456\" LogicalOperator=\"1\" AutomaticUpdate=\"0\">\r\n",
            "  <CONDITION PropertyName=\"artist\" Operator=\"8\" ValueUnit=\"\" ValueLeft=\"Caf&#233; &amp; &quot;Co&quot;\"\r\n",
            "             ValueRight=\"\"/>\r\n",
            "  <CONDITION PropertyName=\"bpm\" Operator=\"5\" ValueUnit=\"0\" ValueLeft=\"12000\"\r\n",
            "             ValueRight=\"12850\"/>\r\n",
            "</NODE>\r\n"
        );
        assert_eq!(xml, expected);
        let back = SmartList::parse(&xml).unwrap();
        assert_eq!(back, list);
        assert_eq!(back.to_xml(), xml);
        assert_eq!(back.conditions[0].left, "Café & \"Co\"");
    }

    #[test]
    fn empty_node_is_self_closing_and_ids_are_signed() {
        let list = SmartList::new("4000000000", MATCH_ANY, Vec::new());
        assert_eq!(list.id, -294967296);
        assert_eq!(
            list.to_xml(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n<NODE Id=\"-294967296\" LogicalOperator=\"2\" AutomaticUpdate=\"0\"/>\r\n"
        );
    }

    #[test]
    fn parse_drops_what_rekordbox_drops() {
        let xml = "<NODE Id='1' LogicalOperator='7'>\
            <CONDITION PropertyName='nope' Operator='1' ValueLeft='x'/>\
            <condition PropertyName='Artist' Operator='12' ValueLeft='x'/>\
            <CONDITION PropertyName='bpm' Operator='8' ValueLeft='1'/>\
            <CONDITION PropertyName='genre' Operator='8' ValueLeft=''/>\
            <CONDITION PropertyName='genre' Operator='1' ValueLeft=''/>\
            <CONDITION PropertyName='myTag' Operator='8' ValueLeft='77'/>\
            </NODE>";
        let list = SmartList::parse(xml).unwrap();
        assert_eq!(list.logical_operator, MATCH_ALL);
        let props: Vec<_> = list.conditions.iter().map(|c| c.property.as_str()).collect();
        assert_eq!(props, ["genre", "myTag"]);
        assert!(SmartList::parse("<SMARTLIST/>").is_none());
        assert!(SmartList::parse("garbage").is_none());
        // No conditions: an empty list.
        let none = SmartList::parse("<NODE Id=\"1\" LogicalOperator=\"1\"/>").unwrap();
        assert!(eval(&none, &[track("1")], &HashMap::new()).is_empty());
    }

    #[test]
    fn text_operators_fold_case_and_accents() {
        let mut a = track("a");
        a.artist = "Beyoncé".into();
        let mut b = track("b");
        b.artist = "DJ Shadow".into();
        let c = track("c"); // no artist
        let tracks = [a, b, c];
        let t = HashMap::new();
        let run = |o, v: &str| eval(&one(Condition::new("artist", o, "", v, "")), &tracks, &t);
        assert_eq!(run(op::IS, "BEYONCE"), ["a"]);
        assert_eq!(run(op::IS_NOT, "beyonce"), ["b", "c"]);
        assert_eq!(run(op::CONTAINS, "shad"), ["b"]);
        assert_eq!(run(op::NOT_CONTAINS, "shad"), ["a"]);
        assert_eq!(run(op::STARTS_WITH, "dj "), ["b"]);
        assert_eq!(run(op::ENDS_WITH, "CÉ"), ["a"]);
        // Empty value: "is" matches empty fields, "is not" every non-empty one.
        assert_eq!(run(op::IS, ""), ["c"]);
        assert_eq!(run(op::IS_NOT, ""), ["a", "b"]);
    }

    #[test]
    fn every_text_property_reads_its_field() {
        for p in PROPERTIES.iter().filter(|p| p.kind == ValueKind::Text) {
            let mut t = track("x");
            let f: &mut String = match p.xml {
                "name" => &mut t.title,
                "artist" => &mut t.artist,
                "album" => &mut t.album,
                "genre" => &mut t.genre,
                "albumArtist" => &mut t.album_artist,
                "comments" => &mut t.comment,
                "producer" => &mut t.composer,
                "mixName" => &mut t.mix_name,
                "remixedBy" => &mut t.remixer,
                "label" => &mut t.label,
                "key" => &mut t.key,
                "originalArtist" => &mut t.original_artist,
                "fileName" => &mut t.file_name,
                other => panic!("{other}"),
            };
            *f = "Needle".into();
            let tracks = [t, track("y")];
            let got = eval(&one(Condition::new(p.xml, op::IS, "", "needle", "")), &tracks, &HashMap::new());
            assert_eq!(got, ["x"], "{}", p.xml);
        }
    }

    #[test]
    fn numbers() {
        let mut a = track("a");
        a.bpm_x100 = Some(12_800);
        a.length_seconds = Some(300);
        a.release_year = Some(1999);
        a.play_count = 3;
        a.rating = 4;
        a.color = Some(5);
        let b = track("b");
        let tracks = [a, b];
        let t = HashMap::new();
        let run = |p, o, l: &str, r: &str| eval(&one(Condition::new(p, o, "", l, r)), &tracks, &t);
        assert_eq!(run("bpm", op::IS, "128", ""), ["a"]);
        assert_eq!(run("bpm", op::IN_RANGE, "127.5", "128.0"), ["a"]);
        assert_eq!(run("bpm", op::LESS, "100", ""), ["b"]);
        assert_eq!(run("duration", op::GREATER, "299", ""), ["a"]);
        assert_eq!(run("year", op::IS_NOT, "1999", ""), ["b"]);
        assert_eq!(run("counter", op::IN_RANGE, "1", "5"), ["a"]);
        assert_eq!(run("rating", op::IS, "4", ""), ["a"]);
        assert_eq!(run("rating", op::IS, "204", ""), ["a"]); // 0..255 scale
        assert_eq!(run("grouping", op::IS, "5", ""), ["a"]);
        assert_eq!(run("grouping", op::IS_NOT, "5", ""), ["b"]);
        // Operators a number does not support match nothing.
        assert!(run("bpm", op::IN_LAST, "1", "").is_empty());
    }

    #[test]
    fn dates() {
        let mut a = track("a");
        a.date_added = "2026-10-13".into();
        a.release_date = "2001-05-04".into();
        a.date_created = "2026-09-30".into();
        let mut b = track("b");
        b.date_added = "2026-10-01".into();
        b.date_created = "2026-08-31".into();
        let c = track("c"); // no dates: never matches
        let tracks = [a, b, c];
        let t = HashMap::new();
        let run = |p, o, u: &str, l: &str, r: &str| eval(&one(Condition::new(p, o, u, l, r)), &tracks, &t);
        assert_eq!(run("stockDate", op::IS, "", "2026-10-13", ""), ["a"]);
        assert_eq!(run("stockDate", op::IS_NOT, "", "2026-10-13", ""), ["b"]);
        assert_eq!(run("stockDate", op::GREATER, "", "2026-10-01", ""), ["a"]);
        assert_eq!(run("stockDate", op::IN_RANGE, "", "2026-09-01", "2026-10-01"), ["b"]);
        assert_eq!(run("dateReleased", op::LESS, "", "2002-01-01", ""), ["a"]);
        // Today is 2026-10-13 local: the last 1 day is today only, the last 13 days start on the 1st.
        assert_eq!(run("stockDate", op::IN_LAST, "day", "1", ""), ["a"]);
        assert_eq!(run("stockDate", op::IN_LAST, "day", "13", ""), ["a", "b"]);
        assert_eq!(run("stockDate", op::NOT_IN_LAST, "day", "1", ""), ["b"]);
        // "In the last 1 month": from the last day of the previous month (2026-09-30) on.
        assert_eq!(run("dateCreated", op::IN_LAST, "month", "1", ""), ["a"]);
        assert_eq!(run("dateCreated", op::IN_LAST, "MONTH", "2", ""), ["a", "b"]);
    }

    #[test]
    fn my_tags_and_any() {
        let tracks = [track("a"), track("b"), track("c")];
        let mut tags = HashMap::new();
        tags.insert(DbId("a".into()), vec![7, 9]);
        tags.insert(DbId("b".into()), vec![9]);
        let has7 = Condition::new("myTag", op::CONTAINS, "", "7", "");
        assert_eq!(eval(&one(has7.clone()), &tracks, &tags), ["a"]);
        assert_eq!(eval(&one(Condition::new("myTag", op::NOT_CONTAINS, "", "7", "")), &tracks, &tags), ["b", "c"]);
        let has9 = Condition::new("myTag", op::CONTAINS, "", "9", "");
        let mut list = SmartList { id: 1, logical_operator: MATCH_ALL, automatic_update: false, conditions: vec![has7, has9] };
        assert_eq!(eval(&list, &tracks, &tags), ["a"]);
        list.logical_operator = MATCH_ANY;
        assert_eq!(eval(&list, &tracks, &tags), ["a", "b"]);
    }

    #[test]
    fn calendar() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 10, 13), 20_739);
        assert_eq!(civil_from_days(20_739), (2026, 10, 13));
        assert_eq!(days_from_civil(2026, 13, 1), days_from_civil(2027, 1, 1));
        assert_eq!(days_from_civil(2026, 3, 0), days_from_civil(2026, 2, 28));
        assert_eq!(date_to_day("2026/10/13", CLOCK), 20_739);
        assert_eq!(date_to_day("2026-10-1", CLOCK), 0);
        assert_eq!(date_to_day("1970-01-01", CLOCK), 0);
        assert_eq!(fold("Straße ＡＢＣ"), "strasse abc");
    }
}
