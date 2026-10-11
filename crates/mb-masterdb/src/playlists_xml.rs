//! `masterPlaylists6.xml`, the playlist side file next to master.db.
//! Spec: `docs/re/ops/playlist-create.md` §5 and `docs/re/masterdb.md` § Side files.
//!
//! Layout (verified on the three fixture files, which [`MasterPlaylists::to_xml`] reproduces byte
//! for byte): UTF-8, CRLF line endings, an empty line after the declaration, 2-space indent, one
//! self-closing `NODE` per playlist with the attributes in the order
//! `Id ParentId Attribute Timestamp Lib_Type CheckType`:
//! ```text
//! <?xml version="1.0" encoding="UTF-8"?>
//!
//! <MASTER_PLAYLIST Version="3.0.0" AutomaticSync="0">
//!   <PRODUCT Name="rekordbox" Version="7.2.19" Company="Pioneer DJ"/>
//!   <PLAYLISTS>
//!     <NODE Id="30D40" ParentId="0" Attribute="0" Timestamp="1791739907631" Lib_Type="0" CheckType="0"/>
//!   </PLAYLISTS>
//! </MASTER_PLAYLIST>
//! ```
//! - `Id` = `djmdPlaylist.ID` in upper-case hex without padding (`200000` → `30D40`).
//! - `ParentId` = 0 for top-level nodes.
//! - `Timestamp` = ms since the epoch of the last change of the list or of one of its tracks.

use crate::{Error, Result};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "masterPlaylists6.xml";

/// The whole file. Unknown attributes are kept so that a parse/write round trip loses nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterPlaylists {
    /// `MASTER_PLAYLIST` attributes in file order (`Version`, `AutomaticSync`).
    pub attributes: Vec<(String, String)>,
    /// `PRODUCT` attributes in file order (`Name`, `Version`, `Company`); empty = no element.
    pub product: Vec<(String, String)>,
    /// `NODE`s in file order.
    pub nodes: Vec<PlaylistXmlNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistXmlNode {
    /// `djmdPlaylist.ID` (written in hex).
    pub id: u64,
    /// Parent playlist ID, 0 for the root.
    // hypothesis: hex like `Id`. Only root children (`ParentId="0"`) have been observed.
    pub parent_id: u64,
    /// `djmdPlaylist.Attribute` (0 playlist, 1 folder, 4 smart list).
    pub attribute: i64,
    /// Last change, ms since the Unix epoch.
    pub timestamp: i64,
    /// Always 0 in the fixtures; meaning open.
    pub lib_type: i64,
    /// Always 0 in the fixtures; meaning open.
    pub check_type: i64,
    /// Attributes beyond the six known ones, in file order.
    pub extra: Vec<(String, String)>,
}

impl PlaylistXmlNode {
    pub fn new(id: u64, parent_id: u64, attribute: i64, timestamp: i64) -> Self {
        Self { id, parent_id, attribute, timestamp, lib_type: 0, check_type: 0, extra: Vec::new() }
    }
}

impl Default for MasterPlaylists {
    /// What rekordbox 7.2.19 writes for a library (header values from the fixtures).
    fn default() -> Self {
        let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        Self {
            attributes: vec![pair("Version", "3.0.0"), pair("AutomaticSync", "0")],
            product: vec![pair("Name", "rekordbox"), pair("Version", "7.2.19"), pair("Company", "Pioneer DJ")],
            nodes: Vec::new(),
        }
    }
}

/// `<dir of master.db>/masterPlaylists6.xml`.
pub fn path_for_db(db_path: &Path) -> PathBuf {
    db_path.parent().unwrap_or(Path::new(".")).join(FILE_NAME)
}

fn bad(msg: impl Into<String>) -> Error {
    Error::PlaylistsXml(msg.into())
}

impl MasterPlaylists {
    pub fn parse(text: &str) -> Result<Self> {
        let mut doc = Self { attributes: Vec::new(), product: Vec::new(), nodes: Vec::new() };
        let mut seen_root = false;
        let mut rest = text.trim_start_matches('\u{feff}');
        while let Some(start) = rest.find('<') {
            let after = &rest[start + 1..];
            let end = after.find('>').ok_or_else(|| bad("unterminated tag"))?;
            let tag = &after[..end];
            rest = &after[end + 1..];
            if tag.starts_with('?') || tag.starts_with('!') || tag.starts_with('/') {
                continue;
            }
            let tag = tag.strip_suffix('/').unwrap_or(tag);
            let (name, attrs) = match tag.find(|c: char| c.is_ascii_whitespace()) {
                Some(i) => (&tag[..i], parse_attributes(&tag[i..])?),
                None => (tag, Vec::new()),
            };
            match name {
                "MASTER_PLAYLIST" => {
                    seen_root = true;
                    doc.attributes = attrs;
                }
                "PRODUCT" => doc.product = attrs,
                "PLAYLISTS" => {}
                "NODE" => doc.nodes.push(node_from_attributes(attrs)?),
                other => return Err(bad(format!("unexpected element <{other}>"))),
            }
        }
        if !seen_root {
            return Err(bad("no <MASTER_PLAYLIST> element"));
        }
        Ok(doc)
    }

    /// rekordbox's layout, CRLF line endings.
    pub fn to_xml(&self) -> String {
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n");
        out.push_str("<MASTER_PLAYLIST");
        push_attributes(&mut out, &self.attributes);
        out.push_str(">\r\n");
        if !self.product.is_empty() {
            out.push_str("  <PRODUCT");
            push_attributes(&mut out, &self.product);
            out.push_str("/>\r\n");
        }
        if self.nodes.is_empty() {
            // hypothesis: an empty list is written self-closing (never observed; every library has
            // at least one playlist after install).
            out.push_str("  <PLAYLISTS/>\r\n");
        } else {
            out.push_str("  <PLAYLISTS>\r\n");
            for n in &self.nodes {
                out.push_str(&format!(
                    "    <NODE Id=\"{:X}\" ParentId=\"{:X}\" Attribute=\"{}\" Timestamp=\"{}\" Lib_Type=\"{}\" CheckType=\"{}\"",
                    n.id, n.parent_id, n.attribute, n.timestamp, n.lib_type, n.check_type
                ));
                push_attributes(&mut out, &n.extra);
                out.push_str("/>\r\n");
            }
            out.push_str("  </PLAYLISTS>\r\n");
        }
        out.push_str("</MASTER_PLAYLIST>\r\n");
        out
    }

    /// Reads the file; `Ok(None)` if it does not exist.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes).map_err(|_| bad("not UTF-8"))?;
                Self::parse(&text).map(Some)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(Error::Io { path: path.display().to_string(), source }),
        }
    }

    /// Writes through a temporary file in the same folder and renames it over `path`, so a reader
    /// never sees a half-written file. The temporary name does not match any rekordbox pattern.
    pub fn write(&self, path: &Path) -> Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        let tmp = dir.join(format!(".{FILE_NAME}.musicbox-tmp"));
        let io = |p: &Path| {
            let p = p.display().to_string();
            move |source| Error::Io { path: p, source }
        };
        std::fs::write(&tmp, self.to_xml()).map_err(io(&tmp))?;
        std::fs::rename(&tmp, path).map_err(io(path))?;
        Ok(())
    }
}

fn node_from_attributes(attrs: Vec<(String, String)>) -> Result<PlaylistXmlNode> {
    let mut node = PlaylistXmlNode::new(0, 0, 0, 0);
    let mut have_id = false;
    let int = |k: &str, v: &str| v.parse::<i64>().map_err(|_| bad(format!("NODE {k}={v:?} is not a number")));
    let hex = |k: &str, v: &str| u64::from_str_radix(v, 16).map_err(|_| bad(format!("NODE {k}={v:?} is not hex")));
    for (k, v) in attrs {
        match k.as_str() {
            "Id" => {
                node.id = hex(&k, &v)?;
                have_id = true;
            }
            "ParentId" => node.parent_id = hex(&k, &v)?,
            "Attribute" => node.attribute = int(&k, &v)?,
            "Timestamp" => node.timestamp = int(&k, &v)?,
            "Lib_Type" => node.lib_type = int(&k, &v)?,
            "CheckType" => node.check_type = int(&k, &v)?,
            _ => node.extra.push((k, v)),
        }
    }
    if !have_id {
        return Err(bad("NODE without Id"));
    }
    Ok(node)
}

fn parse_attributes(mut s: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start();
        if s.is_empty() {
            return Ok(out);
        }
        let eq = s.find('=').ok_or_else(|| bad(format!("attribute without value near {s:?}")))?;
        let key = s[..eq].trim().to_owned();
        let after = s[eq + 1..].trim_start();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or_else(|| bad("unquoted attribute"))?;
        let close = after[1..].find(quote).ok_or_else(|| bad("unterminated attribute value"))?;
        out.push((key, unescape(&after[1..1 + close])));
        s = &after[close + 2..];
    }
}

fn unescape(v: &str) -> String {
    if !v.contains('&') {
        return v.to_owned();
    }
    v.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn push_attributes(out: &mut String, attrs: &[(String, String)]) {
    for (k, v) in attrs {
        let v = v.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
        out.push_str(&format!(" {k}=\"{v}\""));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n<MASTER_PLAYLIST Version=\"3.0.0\" AutomaticSync=\"0\">\r\n  <PRODUCT Name=\"rekordbox\" Version=\"7.2.19\" Company=\"Pioneer DJ\"/>\r\n  <PLAYLISTS>\r\n    <NODE Id=\"3DC1936A\" ParentId=\"0\" Attribute=\"0\" Timestamp=\"1791749159904\" Lib_Type=\"0\" CheckType=\"0\"/>\r\n    <NODE Id=\"30D40\" ParentId=\"0\" Attribute=\"0\" Timestamp=\"1791739907631\" Lib_Type=\"0\" CheckType=\"0\"/>\r\n  </PLAYLISTS>\r\n</MASTER_PLAYLIST>\r\n";

    #[test]
    fn round_trip_and_ids() {
        let doc = MasterPlaylists::parse(SAMPLE).unwrap();
        assert_eq!(doc.nodes.len(), 2);
        assert_eq!(doc.nodes[0].id, 1036096362);
        assert_eq!(doc.nodes[1].id, 200000);
        assert_eq!(doc.nodes[1].timestamp, 1791739907631);
        assert_eq!(doc.to_xml(), SAMPLE);
        let default = MasterPlaylists { nodes: doc.nodes.clone(), ..MasterPlaylists::default() };
        assert_eq!(default.to_xml(), SAMPLE, "default header equals rekordbox 7.2.19's");
    }

    #[test]
    fn unknown_attributes_survive() {
        let text = SAMPLE.replace("CheckType=\"0\"/>\r\n    <NODE Id=\"30D40\"", "CheckType=\"0\" Foo=\"a&amp;b\"/>\r\n    <NODE Id=\"30D40\"");
        let doc = MasterPlaylists::parse(&text).unwrap();
        assert_eq!(doc.nodes[0].extra, vec![("Foo".to_owned(), "a&b".to_owned())]);
        assert_eq!(doc.to_xml(), text);
    }
}
