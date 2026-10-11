//! Generic element tree with rekordbox's XML layout.
//!
//! Writer rules (docs/re/xml.md § Layout), all from `db::DjPlaylistsElement` in rekordbox 7.2.19:
//! - `writeToStream` @0x101054c08: `<?xml version="1.0" encoding="UTF-8"?>`, two newlines, the
//!   root element at indent 0, one newline. Newlines are always CRLF (`juce::newLine`).
//! - `writeElementAsText` @0x10105451c: indent with spaces; each attribute is written as
//!   ` name="value"`. Before an attribute, if the bytes written for attributes on the current line
//!   exceed 60 (`DjPlaylistsWriter::write` @0x101055610 passes 0x3c), a newline is written followed by
//!   `indent + tag-name length + 1` spaces. An element without children ends with `/>`; children
//!   go on their own lines two spaces deeper, then a newline, the indent and `</name>`.
//! - `DjPlaylistsOutputFunctions::escapeIllegalXmlChars` @0x10105494c: `& " ' < >` become
//!   `&amp; &quot; &apos; &lt; &gt;`; other control characters below U+0020 are dropped (in
//!   attributes CR and LF are dropped too, in text they are kept); everything else is written as
//!   raw UTF-8.

use crate::{Error, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

/// CRLF, as `juce::newLine` writes it on every platform.
pub const NEWLINE: &str = "\r\n";
/// rekordbox passes 60 as the attribute line-wrap length.
pub const LINE_WRAP: usize = 60;
pub const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";

/// An element: name, attributes in document order, and children.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<Content>,
}

/// A child of an element. Whitespace-only text between elements is layout and is not kept.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Element(Element),
    Text(String),
}

impl Element {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Appends an attribute (rekordbox's `setAttribute` @0x101054e44 replaces an existing one with
    /// the same name, compared case-insensitively, and appends otherwise).
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        match self
            .attributes
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some(slot) => slot.1 = value,
            None => self.attributes.push((name.to_owned(), value)),
        }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn push(&mut self, child: Element) {
        self.children.push(Content::Element(child));
    }

    /// Child elements, skipping text.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|c| match c {
            Content::Element(e) => Some(e),
            Content::Text(_) => None,
        })
    }

    /// Parses a document and returns its root element. The XML declaration, comments, processing
    /// instructions and the DOCTYPE are skipped; a leading BOM is ignored.
    pub fn parse(xml: &str) -> Result<Element> {
        let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
        let mut reader = Reader::from_str(xml);
        let syntax = |reader: &Reader<&[u8]>, e: &dyn std::fmt::Display| Error::Syntax {
            position: reader.error_position(),
            message: e.to_string(),
        };
        // Open elements; the bottom entry is a virtual document node.
        let mut stack: Vec<Element> = vec![Element::default()];
        // Pending text of the innermost open element (text and entity references arrive as
        // separate events).
        let mut text = String::new();
        loop {
            let event = reader.read_event().map_err(|e| syntax(&reader, &e))?;
            match event {
                Event::Text(t) => text.push_str(&t.into_inner()),
                Event::CData(t) => text.push_str(&t.into_inner()),
                Event::GeneralRef(r) => {
                    let resolved = if r.is_char_ref() {
                        r.resolve_char_ref()
                            .map_err(|e| syntax(&reader, &e))?
                            .map(String::from)
                    } else {
                        quick_xml::escape::resolve_predefined_entity(&r.clone().into_inner())
                            .map(str::to_owned)
                    };
                    let name = r.into_inner();
                    let resolved = resolved.ok_or_else(|| Error::Syntax {
                        position: reader.buffer_position(),
                        message: format!("unknown entity &{name};"),
                    })?;
                    text.push_str(&resolved);
                }
                Event::Start(start) => {
                    flush_text(&mut text, stack.last_mut().expect("document node"));
                    stack.push(start_element(&reader, &start)?);
                }
                Event::Empty(start) => {
                    flush_text(&mut text, stack.last_mut().expect("document node"));
                    let element = start_element(&reader, &start)?;
                    stack.last_mut().expect("document node").push(element);
                }
                Event::End(_) => {
                    let mut element = stack.pop().expect("reader checks end names");
                    flush_text(&mut text, &mut element);
                    stack.last_mut().expect("document node").push(element);
                }
                Event::Eof => break,
                Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_) => {}
            }
        }
        if stack.len() != 1 {
            return Err(Error::Syntax {
                position: reader.buffer_position(),
                message: format!(
                    "unclosed <{}>",
                    stack.last().map_or("", |e| e.name.as_str())
                ),
            });
        }
        let document = stack.pop().expect("document node");
        document
            .children
            .into_iter()
            .find_map(|c| match c {
                Content::Element(e) => Some(e),
                Content::Text(_) => None,
            })
            .ok_or(Error::NoRoot)
    }

    /// The whole document as rekordbox writes it (`writeToFile(file, "", "UTF-8", 60)`).
    pub fn to_document(&self) -> String {
        let mut out = String::with_capacity(64 * 1024);
        out.push_str(XML_HEADER);
        out.push_str(NEWLINE);
        out.push_str(NEWLINE);
        self.write(&mut out, 0);
        out.push_str(NEWLINE);
        out
    }

    /// Writes this element at `indent` (`writeElementAsText` with a non-negative indent).
    pub fn write(&self, out: &mut String, indent: usize) {
        spaces(out, indent);
        out.push('<');
        out.push_str(&self.name);
        let mut line_len = 0;
        for (name, value) in &self.attributes {
            if line_len > LINE_WRAP {
                out.push_str(NEWLINE);
                spaces(out, indent + self.name.chars().count() + 1);
                line_len = 0;
            }
            let start = out.len();
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            escape_into(out, value, true);
            out.push('"');
            line_len += out.len() - start;
        }
        if self.children.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        let mut last_was_text = false;
        for child in &self.children {
            match child {
                Content::Text(t) => {
                    escape_into(out, t, false);
                    last_was_text = true;
                }
                Content::Element(e) => {
                    if !last_was_text {
                        out.push_str(NEWLINE);
                    }
                    e.write(out, if last_was_text { 0 } else { indent + 2 });
                    last_was_text = false;
                }
            }
        }
        if !last_was_text {
            out.push_str(NEWLINE);
            spaces(out, indent);
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push('>');
    }
}

fn flush_text(text: &mut String, into: &mut Element) {
    if !text.trim().is_empty() {
        into.children.push(Content::Text(std::mem::take(text)));
    }
    text.clear();
}

fn start_element(reader: &Reader<&[u8]>, start: &BytesStart) -> Result<Element> {
    let syntax = |e: &dyn std::fmt::Display| Error::Syntax {
        position: reader.error_position(),
        message: e.to_string(),
    };
    let mut element = Element::new(start.name().as_ref());
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| syntax(&e))?;
        let value = quick_xml::escape::unescape(&attribute.value).map_err(|e| syntax(&e))?;
        element
            .attributes
            .push((attribute.key.as_ref().to_owned(), value.into_owned()));
    }
    Ok(element)
}

fn spaces(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n(' ', n));
}

/// rekordbox's escape (`escapeIllegalXmlChars` @0x10105494c). `attribute` is its
/// `changeNewLines` flag: true drops CR and LF, false keeps them. A NUL ends the value, as it ends
/// the C string rekordbox iterates over.
pub fn escape_into(out: &mut String, value: &str, attribute: bool) {
    for c in value.chars() {
        match c {
            '\0' => return,
            '"' => out.push_str("&quot;"),
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&apos;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' | '\r' if !attribute => out.push(c),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_table() {
        let mut out = String::new();
        escape_into(&mut out, "a&b\"c'd<e>f\tg\nh\u{e9}", true);
        assert_eq!(out, "a&amp;b&quot;c&apos;d&lt;e&gt;fgh\u{e9}");
        out.clear();
        escape_into(&mut out, "x\r\ny\0z", false);
        assert_eq!(out, "x\r\ny");
    }

    #[test]
    fn wraps_after_sixty_bytes() {
        let mut e = Element::new("TRACK");
        for (n, v) in [
            ("A", "x".repeat(50)),
            ("B", "y".repeat(3)),
            ("C", "z".to_owned()),
            ("D", "w".to_owned()),
        ] {
            e.set(n, v);
        }
        // ` A="…"` is 55 bytes, then ` B="yyy"` brings the line to 63 > 60, so C wraps.
        let mut out = String::new();
        e.write(&mut out, 4);
        let expected = format!(
            "    <TRACK A=\"{}\" B=\"yyy\"\r\n{} C=\"z\" D=\"w\"/>",
            "x".repeat(50),
            " ".repeat(10)
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn parse_and_write_nested() {
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n<A x=\"1 &amp; 2\">\r\n  <B/>\r\n  <C y=\"&#233;\">\r\n    <D/>\r\n  </C>\r\n</A>\r\n";
        let root = Element::parse(xml).unwrap();
        assert_eq!(root.attr("x"), Some("1 & 2"));
        let c = root.elements().nth(1).unwrap();
        assert_eq!(c.attr("y"), Some("\u{e9}"));
        assert_eq!(
            root.to_document().replace("&#233;", "\u{e9}"),
            xml.replace("&#233;", "\u{e9}")
        );
    }

    #[test]
    fn text_children_are_kept() {
        let root = Element::parse("<A><B>t &lt; u</B></A>").unwrap();
        let b = root.elements().next().unwrap();
        assert_eq!(b.children, vec![Content::Text("t < u".into())]);
        let mut out = String::new();
        root.write(&mut out, 0);
        assert_eq!(out, "<A>\r\n  <B>t &lt; u</B>\r\n</A>");
    }
}
