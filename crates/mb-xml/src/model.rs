//! Typed DJ_PLAYLISTS document. Element and attribute names and their order are rekordbox's
//! (`DjPlaylistsWriter` @0x101054f0c.., docs/re/xml.md § Format). Attributes a file leaves out stay
//! `None` and are not written back; attributes and child elements this model does not know are
//! kept in [`Extra`] and written after the known ones.
//!
//! Numbers are written with rekordbox's formats (`%.02f` for BPM, `%.03f` for seconds), so
//! [`Document::to_xml`] reproduces any file rekordbox wrote. Values with more decimals (other
//! tools) are rounded the way rekordbox would round them.

use crate::dom::{Content, Element};
use crate::{Error, Result};

/// Unknown attributes and child elements, in document order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Extra {
    pub attributes: Vec<(String, String)>,
    pub children: Vec<Content>,
}

impl Extra {
    pub fn is_empty(&self) -> bool {
        self.attributes.is_empty() && self.children.is_empty()
    }
}

/// `<DJ_PLAYLISTS Version="1.0.0">`: the whole file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    /// `Version`; rekordbox writes `1.0.0` (global `xmlVersion`).
    pub version: Option<String>,
    pub product: Option<Product>,
    /// `<COLLECTION Entries="n">` with one TRACK per entry; `None` when the element is absent.
    /// `Entries` is derived from the track count when writing.
    pub collection: Option<Vec<Track>>,
    /// The NODE elements directly under `<PLAYLISTS>` (rekordbox writes one folder named `ROOT`);
    /// `None` when `<PLAYLISTS>` is absent.
    pub playlists: Option<Vec<PlaylistNode>>,
    /// Unknown attributes and children of `<COLLECTION>`.
    pub collection_extra: Extra,
    /// Unknown attributes and children of `<PLAYLISTS>`.
    pub playlists_extra: Extra,
    /// Unknown attributes and children of `<DJ_PLAYLISTS>`.
    pub extra: Extra,
}

/// `<PRODUCT Name="rekordbox" Version="7.2.19" Company="AlphaTheta"/>` (`addProductNode` @0x1010552ac).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Product {
    pub name: Option<String>,
    pub version: Option<String>,
    pub company: Option<String>,
    pub extra: Extra,
}

/// `<COLLECTION><TRACK …>` (`addTrackNode` @0x101055910). Field order is the attribute order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    /// `TrackID`: `djmdContent.ID` written as a signed 32-bit integer (see [`crate::export::xml_id`]).
    pub track_id: Option<i64>,
    pub name: Option<String>,
    pub artist: Option<String>,
    pub composer: Option<String>,
    pub album: Option<String>,
    /// rekordbox writes the track colour's name here (`djmdColor.Commnt`, e.g. `Pink`); hypothesis.
    pub grouping: Option<String>,
    pub genre: Option<String>,
    /// Localised file-type label (`MP3 File`, German `Mp3-Datei `), see [`crate::export::kind_label`].
    pub kind: Option<String>,
    /// `Size`, bytes (rekordbox writes it as a signed 32-bit int).
    pub size: Option<i64>,
    /// `TotalTime`, whole seconds.
    pub total_time: Option<u32>,
    pub disc_number: Option<u32>,
    pub track_number: Option<u32>,
    pub year: Option<u32>,
    /// `AverageBpm`, written `%.02f`.
    pub average_bpm: Option<f64>,
    /// `DateAdded`, `YYYY-MM-DD` (`djmdContent.StockDate`).
    pub date_added: Option<String>,
    /// `BitRate`, kbit/s (0 for lossless ALAC).
    pub bit_rate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub comments: Option<String>,
    pub play_count: Option<u32>,
    /// `Rating`: stars × 51 (0, 51, 102, 153, 204, 255).
    pub rating: Option<u8>,
    /// `Location`: `file://localhost` + percent-encoded path, see [`crate::export::location_to_uri`].
    pub location: Option<String>,
    pub remixer: Option<String>,
    /// `Tonality`: `djmdKey.ScaleName` (e.g. `Am`).
    pub tonality: Option<String>,
    pub label: Option<String>,
    /// `Mix` (`djmdContent.Subtitle`; hypothesis, empty in the fixture).
    pub mix: Option<String>,
    /// `Colour` as `0xRRGGBB`; only written for the eight track colours.
    pub colour: Option<u32>,
    pub tempos: Vec<Tempo>,
    pub position_marks: Vec<PositionMark>,
    pub extra: Extra,
}

/// `<TEMPO Inizio="0.019" Bpm="120.00" Metro="4/4" Battito="1"/>` (`addTempoNode` @0x101057074).
#[derive(Debug, Clone, PartialEq)]
pub struct Tempo {
    /// `Inizio`: start in seconds, written `%.03f`.
    pub inizio: f64,
    /// `Bpm`, written `%.02f`.
    pub bpm: f64,
    /// `Metro`: time signature; rekordbox always writes `4/4`.
    pub metro: String,
    /// `Battito`: beat in bar (1..=4) of the beat at `inizio`.
    pub battito: u32,
    pub extra: Extra,
}

/// `<POSITION_MARK Name="" Type="0" Start="1.234" Num="-1"/>` (`addPositionMarkNode` @0x1010574b0).
#[derive(Debug, Clone, PartialEq)]
pub struct PositionMark {
    pub name: Option<String>,
    /// `Type`: 0 cue, 4 loop are what rekordbox writes; 1 fade-in, 2 fade-out, 3 load are read.
    pub mark_type: u8,
    /// `Start`, seconds, written `%.03f`.
    pub start: f64,
    /// `End`, seconds; loops only.
    pub end: Option<f64>,
    /// `Num`: hot cue 0 = A … 7 = H, -1 = memory cue.
    pub num: i32,
    /// `Red`/`Green`/`Blue`; rekordbox writes them for hot cues only.
    pub rgb: Option<[u8; 3]>,
    pub extra: Extra,
}

/// `KeyType` of a playlist: how its `<TRACK Key>` refers to the collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    /// 0: `Key` is a `TrackID`. rekordbox always writes this.
    TrackId,
    /// 1: `Key` is a `Location` URI.
    Location,
}

/// A `<NODE>` of the playlist tree (`addPlaylistsNode` @0x101056400, `addPlaylistNode` @0x1010566cc).
/// `Count` and `Entries` are derived from the children when writing.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaylistNode {
    /// `Type="0"`.
    Folder {
        name: String,
        children: Vec<PlaylistNode>,
        extra: Extra,
    },
    /// `Type="1"`.
    Playlist {
        name: String,
        key_type: Option<KeyType>,
        entries: Vec<PlaylistEntry>,
        extra: Extra,
    },
}

/// `<TRACK Key="…"/>` inside a playlist.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlaylistEntry {
    pub key: String,
    pub extra: Extra,
}

impl PlaylistEntry {
    pub fn track_id(id: i64) -> Self {
        Self {
            key: id.to_string(),
            extra: Extra::default(),
        }
    }
}

impl PlaylistNode {
    pub fn name(&self) -> &str {
        match self {
            Self::Folder { name, .. } | Self::Playlist { name, .. } => name,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Reading

/// Attribute access that records which attributes were consumed, so the rest go to [`Extra`].
struct Attrs<'a> {
    element: &'a Element,
    used: Vec<bool>,
}

impl<'a> Attrs<'a> {
    fn new(element: &'a Element) -> Self {
        Self {
            element,
            used: vec![false; element.attributes.len()],
        }
    }

    fn take(&mut self, name: &str) -> Option<&'a str> {
        let i = self
            .element
            .attributes
            .iter()
            .position(|(n, _)| n == name)?;
        self.used[i] = true;
        Some(self.element.attributes[i].1.as_str())
    }

    fn string(&mut self, name: &str) -> Option<String> {
        self.take(name).map(str::to_owned)
    }

    fn bad(&self, name: &str, value: &str, message: impl Into<String>) -> Error {
        Error::BadAttribute {
            element: self.element.name.clone(),
            attribute: name.to_owned(),
            value: value.to_owned(),
            message: message.into(),
        }
    }

    fn parsed<T: std::str::FromStr>(&mut self, name: &str) -> Result<Option<T>>
    where
        T::Err: std::fmt::Display,
    {
        match self.take(name) {
            None => Ok(None),
            Some(v) => v
                .trim()
                .parse::<T>()
                .map(Some)
                .map_err(|e| self.bad(name, v, e.to_string())),
        }
    }

    fn required<T: std::str::FromStr>(&mut self, name: &str) -> Result<T>
    where
        T::Err: std::fmt::Display,
    {
        self.parsed(name)?.ok_or_else(|| Error::MissingAttribute {
            element: self.element.name.clone(),
            attribute: name.to_owned(),
        })
    }

    /// `0xRRGGBB` (rekordbox compares the eight colour strings case-insensitively on import).
    fn colour(&mut self, name: &str) -> Result<Option<u32>> {
        let Some(v) = self.take(name) else {
            return Ok(None);
        };
        let hex = v
            .strip_prefix("0x")
            .or_else(|| v.strip_prefix("0X"))
            .unwrap_or(v);
        u32::from_str_radix(hex, 16)
            .ok()
            .filter(|c| *c <= 0xff_ffff)
            .map(Some)
            .ok_or_else(|| self.bad(name, v, "expected 0xRRGGBB"))
    }

    /// Unconsumed attributes plus the given unknown children.
    fn extra(self, children: Vec<Content>) -> Extra {
        Extra {
            attributes: self
                .element
                .attributes
                .iter()
                .zip(&self.used)
                .filter(|(_, used)| !**used)
                .map(|(a, _)| a.clone())
                .collect(),
            children,
        }
    }
}

fn expect_name(e: &Element, name: &str) -> Result<()> {
    if e.name == name {
        Ok(())
    } else {
        Err(Error::UnexpectedElement {
            expected: name.to_owned(),
            found: e.name.clone(),
        })
    }
}

impl Document {
    /// Parses a rekordbox.xml document.
    pub fn parse(xml: &str) -> Result<Self> {
        Self::from_element(&Element::parse(xml)?)
    }

    pub fn from_element(root: &Element) -> Result<Self> {
        expect_name(root, "DJ_PLAYLISTS")?;
        let mut attrs = Attrs::new(root);
        let version = attrs.string("Version");
        let (mut product, mut collection, mut playlists) = (None, None, None);
        let (mut collection_extra, mut playlists_extra) = (Extra::default(), Extra::default());
        let mut unknown = Vec::new();
        for child in &root.children {
            match child {
                Content::Element(e) if e.name == "PRODUCT" && product.is_none() => {
                    product = Some(Product::from_element(e)?);
                }
                Content::Element(e) if e.name == "COLLECTION" && collection.is_none() => {
                    let mut a = Attrs::new(e);
                    a.take("Entries");
                    let mut tracks = Vec::new();
                    let mut other = Vec::new();
                    for c in &e.children {
                        match c {
                            Content::Element(t) if t.name == "TRACK" => {
                                tracks.push(Track::from_element(t)?)
                            }
                            c => other.push(c.clone()),
                        }
                    }
                    collection = Some(tracks);
                    collection_extra = a.extra(other);
                }
                Content::Element(e) if e.name == "PLAYLISTS" && playlists.is_none() => {
                    let a = Attrs::new(e);
                    let mut nodes = Vec::new();
                    let mut other = Vec::new();
                    for c in &e.children {
                        match c {
                            Content::Element(n) if n.name == "NODE" => {
                                nodes.push(PlaylistNode::from_element(n)?)
                            }
                            c => other.push(c.clone()),
                        }
                    }
                    playlists = Some(nodes);
                    playlists_extra = a.extra(other);
                }
                other => unknown.push(other.clone()),
            }
        }
        Ok(Self {
            version,
            product,
            collection,
            playlists,
            collection_extra,
            playlists_extra,
            extra: attrs.extra(unknown),
        })
    }

    /// The tree rekordbox would build for this document.
    pub fn to_element(&self) -> Element {
        let mut root = Element::new("DJ_PLAYLISTS");
        if let Some(v) = &self.version {
            root.set("Version", v.as_str());
        }
        push_extra_attributes(&mut root, &self.extra);
        if let Some(p) = &self.product {
            root.push(p.to_element());
        }
        if let Some(tracks) = &self.collection {
            let mut c = Element::new("COLLECTION");
            c.set("Entries", tracks.len().to_string());
            push_extra_attributes(&mut c, &self.collection_extra);
            for t in tracks {
                c.push(t.to_element());
            }
            c.children
                .extend(self.collection_extra.children.iter().cloned());
            root.push(c);
        }
        if let Some(nodes) = &self.playlists {
            let mut p = Element::new("PLAYLISTS");
            push_extra_attributes(&mut p, &self.playlists_extra);
            for n in nodes {
                p.push(n.to_element(true));
            }
            p.children
                .extend(self.playlists_extra.children.iter().cloned());
            root.push(p);
        }
        root.children.extend(self.extra.children.iter().cloned());
        root
    }

    /// The document as rekordbox writes it: header, CRLF line ends, its indentation and wrapping.
    pub fn to_xml(&self) -> String {
        self.to_element().to_document()
    }

    pub fn tracks(&self) -> &[Track] {
        self.collection.as_deref().unwrap_or_default()
    }
}

fn push_extra_attributes(e: &mut Element, extra: &Extra) {
    for (n, v) in &extra.attributes {
        e.attributes.push((n.clone(), v.clone()));
    }
}

impl Product {
    /// What rekordbox 7 writes: `Name="rekordbox" Version="<app version>" Company="AlphaTheta"`.
    pub fn rekordbox(version: &str) -> Self {
        Self {
            name: Some("rekordbox".into()),
            version: Some(version.into()),
            company: Some("AlphaTheta".into()),
            extra: Extra::default(),
        }
    }

    fn from_element(e: &Element) -> Result<Self> {
        let mut a = Attrs::new(e);
        Ok(Self {
            name: a.string("Name"),
            version: a.string("Version"),
            company: a.string("Company"),
            extra: a.extra(e.children.clone()),
        })
    }

    fn to_element(&self) -> Element {
        let mut e = Element::new("PRODUCT");
        for (n, v) in [
            ("Name", &self.name),
            ("Version", &self.version),
            ("Company", &self.company),
        ] {
            if let Some(v) = v {
                e.set(n, v.as_str());
            }
        }
        push_extra_attributes(&mut e, &self.extra);
        e.children.extend(self.extra.children.iter().cloned());
        e
    }
}

impl Track {
    pub fn from_element(e: &Element) -> Result<Self> {
        expect_name(e, "TRACK")?;
        let mut a = Attrs::new(e);
        let mut t = Track {
            track_id: a.parsed("TrackID")?,
            name: a.string("Name"),
            artist: a.string("Artist"),
            composer: a.string("Composer"),
            album: a.string("Album"),
            grouping: a.string("Grouping"),
            genre: a.string("Genre"),
            kind: a.string("Kind"),
            size: a.parsed("Size")?,
            total_time: a.parsed("TotalTime")?,
            disc_number: a.parsed("DiscNumber")?,
            track_number: a.parsed("TrackNumber")?,
            year: a.parsed("Year")?,
            average_bpm: a.parsed("AverageBpm")?,
            date_added: a.string("DateAdded"),
            bit_rate: a.parsed("BitRate")?,
            sample_rate: a.parsed("SampleRate")?,
            comments: a.string("Comments"),
            play_count: a.parsed("PlayCount")?,
            rating: a.parsed("Rating")?,
            location: a.string("Location"),
            remixer: a.string("Remixer"),
            tonality: a.string("Tonality"),
            label: a.string("Label"),
            mix: a.string("Mix"),
            colour: a.colour("Colour")?,
            ..Track::default()
        };
        let mut unknown = Vec::new();
        for child in &e.children {
            match child {
                Content::Element(c) if c.name == "TEMPO" && t.position_marks.is_empty() => {
                    t.tempos.push(Tempo::from_element(c)?);
                }
                Content::Element(c) if c.name == "POSITION_MARK" => {
                    t.position_marks.push(PositionMark::from_element(c)?);
                }
                other => unknown.push(other.clone()),
            }
        }
        t.extra = a.extra(unknown);
        Ok(t)
    }

    pub fn to_element(&self) -> Element {
        let mut e = Element::new("TRACK");
        let int = |v: &Option<u32>| v.map(|v| v.to_string());
        let fields: [(&str, Option<String>); 26] = [
            ("TrackID", self.track_id.map(|v| v.to_string())),
            ("Name", self.name.clone()),
            ("Artist", self.artist.clone()),
            ("Composer", self.composer.clone()),
            ("Album", self.album.clone()),
            ("Grouping", self.grouping.clone()),
            ("Genre", self.genre.clone()),
            ("Kind", self.kind.clone()),
            ("Size", self.size.map(|v| v.to_string())),
            ("TotalTime", int(&self.total_time)),
            ("DiscNumber", int(&self.disc_number)),
            ("TrackNumber", int(&self.track_number)),
            ("Year", int(&self.year)),
            ("AverageBpm", self.average_bpm.map(fmt_bpm)),
            ("DateAdded", self.date_added.clone()),
            ("BitRate", int(&self.bit_rate)),
            ("SampleRate", int(&self.sample_rate)),
            ("Comments", self.comments.clone()),
            ("PlayCount", int(&self.play_count)),
            ("Rating", self.rating.map(|v| v.to_string())),
            ("Location", self.location.clone()),
            ("Remixer", self.remixer.clone()),
            ("Tonality", self.tonality.clone()),
            ("Label", self.label.clone()),
            ("Mix", self.mix.clone()),
            ("Colour", self.colour.map(|c| format!("0x{c:06X}"))),
        ];
        for (n, v) in fields {
            if let Some(v) = v {
                e.set(n, v);
            }
        }
        push_extra_attributes(&mut e, &self.extra);
        for t in &self.tempos {
            e.push(t.to_element());
        }
        for m in &self.position_marks {
            e.push(m.to_element());
        }
        e.children.extend(self.extra.children.iter().cloned());
        e
    }
}

/// `%.02f` (BPM).
pub fn fmt_bpm(v: f64) -> String {
    format!("{v:.2}")
}

/// `%.03f` (seconds). Rust's fixed-precision formatting rounds the exact binary value half-to-even,
/// like the C library's `vsnprintf` that `juce::String::formattedRaw` calls.
pub fn fmt_seconds(v: f64) -> String {
    format!("{v:.3}")
}

impl Tempo {
    fn from_element(e: &Element) -> Result<Self> {
        let mut a = Attrs::new(e);
        Ok(Self {
            inizio: a.required("Inizio")?,
            bpm: a.required("Bpm")?,
            metro: a.required("Metro")?,
            battito: a.required("Battito")?,
            extra: a.extra(e.children.clone()),
        })
    }

    fn to_element(&self) -> Element {
        let mut e = Element::new("TEMPO");
        e.set("Inizio", fmt_seconds(self.inizio));
        e.set("Bpm", fmt_bpm(self.bpm));
        e.set("Metro", self.metro.as_str());
        e.set("Battito", self.battito.to_string());
        push_extra_attributes(&mut e, &self.extra);
        e.children.extend(self.extra.children.iter().cloned());
        e
    }
}

impl PositionMark {
    fn from_element(e: &Element) -> Result<Self> {
        let mut a = Attrs::new(e);
        let name = a.string("Name");
        let mark_type = a.required("Type")?;
        let start = a.required("Start")?;
        let end = a.parsed("End")?;
        let num = a.required("Num")?;
        let rgb = match (
            a.parsed::<u8>("Red")?,
            a.parsed::<u8>("Green")?,
            a.parsed::<u8>("Blue")?,
        ) {
            (Some(r), Some(g), Some(b)) => Some([r, g, b]),
            (None, None, None) => None,
            _ => {
                return Err(Error::MissingAttribute {
                    element: e.name.clone(),
                    attribute: "Red/Green/Blue (all three or none)".into(),
                });
            }
        };
        Ok(Self {
            name,
            mark_type,
            start,
            end,
            num,
            rgb,
            extra: a.extra(e.children.clone()),
        })
    }

    fn to_element(&self) -> Element {
        let mut e = Element::new("POSITION_MARK");
        if let Some(n) = &self.name {
            e.set("Name", n.as_str());
        }
        e.set("Type", self.mark_type.to_string());
        e.set("Start", fmt_seconds(self.start));
        if let Some(end) = self.end {
            e.set("End", fmt_seconds(end));
        }
        e.set("Num", self.num.to_string());
        if let Some([r, g, b]) = self.rgb {
            e.set("Red", r.to_string());
            e.set("Green", g.to_string());
            e.set("Blue", b.to_string());
        }
        push_extra_attributes(&mut e, &self.extra);
        e.children.extend(self.extra.children.iter().cloned());
        e
    }
}

impl KeyType {
    fn code(self) -> u8 {
        match self {
            Self::TrackId => 0,
            Self::Location => 1,
        }
    }
}

impl PlaylistNode {
    fn from_element(e: &Element) -> Result<Self> {
        expect_name(e, "NODE")?;
        let mut a = Attrs::new(e);
        let name = a.string("Name").unwrap_or_default();
        let node_type: u8 = a.required("Type")?;
        match node_type {
            0 => {
                a.take("Count");
                let mut children = Vec::new();
                let mut unknown = Vec::new();
                for c in &e.children {
                    match c {
                        Content::Element(n) if n.name == "NODE" => {
                            children.push(Self::from_element(n)?)
                        }
                        other => unknown.push(other.clone()),
                    }
                }
                Ok(Self::Folder {
                    name,
                    children,
                    extra: a.extra(unknown),
                })
            }
            1 => {
                a.take("Entries");
                let key_type = match a.parsed::<u8>("KeyType")? {
                    None => None,
                    Some(0) => Some(KeyType::TrackId),
                    Some(1) => Some(KeyType::Location),
                    Some(other) => {
                        return Err(a.bad("KeyType", &other.to_string(), "expected 0 or 1"));
                    }
                };
                let mut entries = Vec::new();
                let mut unknown = Vec::new();
                for c in &e.children {
                    match c {
                        Content::Element(t) if t.name == "TRACK" => {
                            let mut ta = Attrs::new(t);
                            let key = ta.string("Key").ok_or_else(|| Error::MissingAttribute {
                                element: "TRACK (playlist entry)".into(),
                                attribute: "Key".into(),
                            })?;
                            entries.push(PlaylistEntry {
                                key,
                                extra: ta.extra(t.children.clone()),
                            });
                        }
                        other => unknown.push(other.clone()),
                    }
                }
                Ok(Self::Playlist {
                    name,
                    key_type,
                    entries,
                    extra: a.extra(unknown),
                })
            }
            other => Err(a.bad(
                "Type",
                &other.to_string(),
                "expected 0 (folder) or 1 (playlist)",
            )),
        }
    }

    /// `top_level`: nodes directly under PLAYLISTS (the `ROOT` folder) put `Type` before `Name`,
    /// as `addPlaylistsNode` @0x101056400 does; all deeper nodes start with `Name`.
    fn to_element(&self, top_level: bool) -> Element {
        let mut e = Element::new("NODE");
        match self {
            Self::Folder {
                name,
                children,
                extra,
            } => {
                if top_level {
                    e.set("Type", "0");
                    e.set("Name", name.as_str());
                } else {
                    e.set("Name", name.as_str());
                    e.set("Type", "0");
                }
                e.set("Count", children.len().to_string());
                push_extra_attributes(&mut e, extra);
                for c in children {
                    e.push(c.to_element(false));
                }
                e.children.extend(extra.children.iter().cloned());
            }
            Self::Playlist {
                name,
                key_type,
                entries,
                extra,
            } => {
                if top_level {
                    e.set("Type", "1");
                    e.set("Name", name.as_str());
                } else {
                    e.set("Name", name.as_str());
                    e.set("Type", "1");
                }
                if let Some(k) = key_type {
                    e.set("KeyType", k.code().to_string());
                }
                e.set("Entries", entries.len().to_string());
                push_extra_attributes(&mut e, extra);
                for entry in entries {
                    let mut t = Element::new("TRACK");
                    t.set("Key", entry.key.as_str());
                    push_extra_attributes(&mut t, &entry.extra);
                    t.children.extend(entry.extra.children.iter().cloned());
                    e.push(t);
                }
                e.children.extend(extra.children.iter().cloned());
            }
        }
        e
    }
}
