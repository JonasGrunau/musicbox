//! rekordbox.xml ("File → Export Collection in xml format") reader and writer. Spec: `docs/re/xml.md`.
//!
//! Three layers:
//! - [`dom`]: a generic element tree. [`Element::parse`] reads any XML through quick-xml;
//!   [`Element::to_document`] writes it with rekordbox's own layout rules (`DjPlaylistsElement`
//!   @0x10105451c: 2-space indent, attributes wrapped after 60 bytes, CRLF, its escape table).
//! - [`model`]: the typed DJ_PLAYLISTS document ([`Document`], [`Track`], [`Tempo`],
//!   [`PositionMark`], [`PlaylistNode`]), converted to and from the tree. Unknown attributes and
//!   elements are kept in [`Extra`] so nothing is dropped.
//! - [`export`]: how rekordbox derives XML values from its library (`DjPlaylistsWriter`
//!   @0x101054f0c..): number formats, the TEMPO reduction of a PQTZ beat grid, cue kinds, colours,
//!   ratings, `Location` URIs and the localised `Kind` names.
//!
//! [`Document::parse`] followed by [`Document::to_xml`] reproduces a rekordbox-written file byte for
//! byte (verified on fixtures/oracle-a, see `tests/fixture.rs`).

pub mod dom;
pub mod export;
pub mod model;

pub use dom::{Content, Element};
pub use model::{
    Document, Extra, KeyType, PlaylistEntry, PlaylistNode, PositionMark, Product, Tempo, Track,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("XML syntax error at byte {position}: {message}")]
    Syntax { position: u64, message: String },
    #[error("document has no root element")]
    NoRoot,
    #[error("expected <{expected}>, found <{found}>")]
    UnexpectedElement { expected: String, found: String },
    #[error("<{element}> is missing attribute {attribute}")]
    MissingAttribute { element: String, attribute: String },
    #[error("<{element}> attribute {attribute}={value:?}: {message}")]
    BadAttribute {
        element: String,
        attribute: String,
        value: String,
        message: String,
    },
    #[error("<{element}> contains text {text:?}; rekordbox.xml has none")]
    UnexpectedText { element: String, text: String },
}

pub type Result<T> = std::result::Result<T, Error>;
