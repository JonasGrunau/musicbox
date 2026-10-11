//! Op: edit one metadata field of a track.
//!
//! **hypothesis (not traced):** the 2026-10-11 session did not edit any track field
//! (`docs/re/ops/INDEX.md` § Coverage gaps). The op is assembled from traced building blocks:
//! - the field is written with UPDATE-COL (`common.md`: one builder update of one column, one
//!   USN; numbers bound as numbers, VARCHAR columns as text);
//! - a name field (artist, album, genre, label, remixer, composer, original artist, key) first
//!   runs the import lookup-or-create (`import-track.md`; one USN per created row) and then writes
//!   the ID column, as analysis does for `KeyID` (`analyse-track.md`);
//! - the replaced row then gets the orphan step of its table (analysis runs it for `djmdKey`
//!   after every `KeyID` write; one USN only if it is orphaned);
//! - finally `TrackInfoUpdated += 1` (COUNTER macro, one USN), as analysis does after it changes
//!   track info. The USB export reads this counter to decide what to re-send.
//!
//! An edit to the current value writes nothing.

use crate::lookup::{NameTable, find_or_create, find_or_create_album, name_of};
use crate::{Command, DbId, Effect, Error, Library, Result, xml_sync};
use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::{CounterColumn, OrphanTable};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackEdit {
    Title(String),
    Artist(String),
    Album(String),
    Genre(String),
    Label(String),
    Remixer(String),
    Composer(String),
    OriginalArtist(String),
    /// Classic key name stored verbatim as `djmdKey.ScaleName`; `""` clears it (`KeyID = "0"`).
    Key(String),
    Comment(String),
    Lyricist(String),
    /// 0…5 stars.
    Rating(u8),
    /// Colour slot 1…8 (`djmdColor`), 0 = none.
    Color(u8),
    TrackNo(u32),
    DiscNo(u32),
    ReleaseYear(u32),
}

/// How a field is stored.
enum Field {
    /// A plain column: (column, value).
    Direct(&'static str, Value),
    /// A lookup ID column: (ID column, lookup table, name).
    Name(&'static str, NameTable, String),
    Album(String),
}

impl TrackEdit {
    fn field(&self) -> Field {
        use TrackEdit::*;
        let text = |s: &str| Value::Text(s.to_owned());
        match self {
            Title(s) => Field::Direct("Title", text(s)),
            Comment(s) => Field::Direct("Commnt", text(s)),
            Lyricist(s) => Field::Direct("Lyricist", text(s)),
            Rating(r) => Field::Direct("Rating", Value::Integer(i64::from(*r))),
            // `ColorID` is VARCHAR: `"0"` at import, the slot number as text otherwise.
            Color(c) => Field::Direct("ColorID", Value::Text(c.to_string())),
            TrackNo(n) => Field::Direct("TrackNo", Value::Integer(i64::from(*n))),
            DiscNo(n) => Field::Direct("DiscNo", Value::Integer(i64::from(*n))),
            ReleaseYear(n) => Field::Direct("ReleaseYear", Value::Integer(i64::from(*n))),
            Artist(s) => Field::Name("ArtistID", NameTable::Artist, s.clone()),
            Remixer(s) => Field::Name("RemixerID", NameTable::Artist, s.clone()),
            Composer(s) => Field::Name("ComposerID", NameTable::Artist, s.clone()),
            OriginalArtist(s) => Field::Name("OrgArtistID", NameTable::Artist, s.clone()),
            Genre(s) => Field::Name("GenreID", NameTable::Genre, s.clone()),
            Label(s) => Field::Name("LabelID", NameTable::Label, s.clone()),
            Key(s) => Field::Name("KeyID", NameTable::Key, s.clone()),
            Album(s) => Field::Album(s.clone()),
        }
    }

    fn validate(&self) -> Result<()> {
        match self {
            TrackEdit::Rating(r) if *r > 5 => Err(Error::Invalid(format!("rating {r} is not 0…5"))),
            TrackEdit::Color(c) if *c > 8 => Err(Error::Invalid(format!("colour {c} is not 0…8"))),
            _ => Ok(()),
        }
    }

    /// The same kind of edit with another value (for the inverse).
    fn with_value(&self, v: CurrentValue) -> TrackEdit {
        use TrackEdit::*;
        let (s, n) = match v {
            CurrentValue::Text(s) => (s, 0),
            CurrentValue::Number(n) => (String::new(), n),
        };
        let small = |n: i64| n.clamp(0, i64::from(u8::MAX)) as u8;
        let unsigned = |n: i64| n.clamp(0, i64::from(u32::MAX)) as u32;
        match self {
            Title(_) => Title(s),
            Artist(_) => Artist(s),
            Album(_) => Album(s),
            Genre(_) => Genre(s),
            Label(_) => Label(s),
            Remixer(_) => Remixer(s),
            Composer(_) => Composer(s),
            OriginalArtist(_) => OriginalArtist(s),
            Key(_) => Key(s),
            Comment(_) => Comment(s),
            Lyricist(_) => Lyricist(s),
            Rating(_) => Rating(small(n)),
            Color(_) => Color(small(n)),
            TrackNo(_) => TrackNo(unsigned(n)),
            DiscNo(_) => DiscNo(unsigned(n)),
            ReleaseYear(_) => ReleaseYear(unsigned(n)),
        }
    }
}

enum CurrentValue {
    Text(String),
    Number(i64),
}

fn orphan_tables(t: NameTable) -> &'static [OrphanTable] {
    match t {
        NameTable::Artist => &[OrphanTable::Artist],
        NameTable::Genre => &[OrphanTable::Genre],
        NameTable::Label => &[OrphanTable::Label],
        NameTable::Key => &[OrphanTable::Key],
    }
}

pub(crate) fn edit_track(lib: &mut Library, id: &DbId, edit: &TrackEdit) -> Result<(Effect, Option<Command>)> {
    edit.validate()?;
    let exists = lib
        .connection()
        .query_row("SELECT 1 FROM djmdContent WHERE ID = ?1 AND rb_local_deleted = 0", [id.as_str()], |_| Ok(()))
        .is_ok();
    if !exists {
        return Err(Error::NotFound { what: "track", id: id.0.clone() });
    }
    let field = edit.field();
    let tx = lib.writer.transaction()?;
    let column_value = |col: &str| -> Result<Value> {
        Ok(tx.connection().query_row(&format!("SELECT {col} FROM djmdContent WHERE ID = ?1"), [id.as_str()], |r| r.get::<_, Value>(0))?)
    };
    let (old, changed) = match &field {
        Field::Direct(col, new) => {
            let cur = column_value(col)?;
            let old = match &cur {
                Value::Integer(n) => CurrentValue::Number(*n),
                Value::Real(f) => CurrentValue::Number(*f as i64),
                Value::Text(s) if matches!(new, Value::Integer(_)) || *col == "ColorID" => CurrentValue::Number(s.trim().parse().unwrap_or(0)),
                Value::Text(s) => CurrentValue::Text(s.clone()),
                _ => if matches!(new, Value::Text(_)) && *col != "ColorID" { CurrentValue::Text(String::new()) } else { CurrentValue::Number(0) },
            };
            let same = match (&old, new) {
                (CurrentValue::Number(a), Value::Integer(b)) => a == b,
                (CurrentValue::Number(a), Value::Text(b)) => a.to_string() == *b,
                (CurrentValue::Text(a), Value::Text(b)) => a == b,
                _ => false,
            } && !matches!(cur, Value::Null);
            if same {
                (old, false)
            } else {
                tx.update("djmdContent", "ID", Value::Text(id.0.clone()), &[(col, new.clone())])?;
                (old, true)
            }
        }
        Field::Name(col, table, name) => {
            let cur_id: Option<String> = match column_value(col)? {
                Value::Text(s) => Some(s),
                Value::Integer(n) => Some(n.to_string()),
                _ => None,
            };
            let cur_id = cur_id.filter(|s| !(s.is_empty() || (*table == NameTable::Key && s == "0")));
            let old_name = match &cur_id {
                Some(i) => name_of(&tx, table.table(), i)?.unwrap_or_default(),
                None => String::new(),
            };
            if old_name == *name && cur_id.is_some() != name.is_empty() {
                (CurrentValue::Text(old_name), false)
            } else {
                let new_id = if name.is_empty() { None } else { Some(find_or_create(&tx, *table, name)?) };
                let value = match (&new_id, table) {
                    (Some(i), _) => Value::Text(i.clone()),
                    // Import writes KeyID "0" (never NULL) without a key; other IDs stay NULL.
                    (None, NameTable::Key) => Value::Text("0".into()),
                    (None, _) => Value::Null,
                };
                tx.update("djmdContent", "ID", Value::Text(id.0.clone()), &[(col, value)])?;
                if cur_id.is_some() && cur_id != new_id {
                    for t in orphan_tables(*table) {
                        tx.cleanup_orphan_table(*t)?;
                    }
                }
                (CurrentValue::Text(old_name), true)
            }
        }
        Field::Album(name) => {
            let cur_id: Option<String> = match column_value("AlbumID")? {
                Value::Text(s) if !s.is_empty() => Some(s),
                _ => None,
            };
            let (old_name, album_artist): (String, Option<String>) = match &cur_id {
                Some(i) => tx
                    .connection()
                    .query_row("SELECT Name, AlbumArtistID FROM djmdAlbum WHERE ID = ?1", [i], |r| {
                        Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?))
                    })
                    .unwrap_or_default(),
                None => (String::new(), None),
            };
            if old_name == *name && cur_id.is_some() != name.is_empty() {
                (CurrentValue::Text(old_name), false)
            } else {
                // hypothesis: the new album keeps the album artist of the track's current album.
                let new_id = if name.is_empty() { None } else { Some(find_or_create_album(&tx, name, album_artist.as_deref())?) };
                let value = new_id.clone().map_or(Value::Null, Value::Text);
                tx.update("djmdContent", "ID", Value::Text(id.0.clone()), &[("AlbumID", value)])?;
                if cur_id.is_some() && cur_id != new_id {
                    // An orphaned album may orphan its album artist in turn.
                    tx.cleanup_orphan_table(OrphanTable::Album)?;
                    tx.cleanup_orphan_table(OrphanTable::Artist)?;
                }
                (CurrentValue::Text(old_name), true)
            }
        }
    };
    if !changed {
        return Ok((Effect::Unchanged, None));
    }
    tx.increment_counter(id.as_str(), CounterColumn::TrackInfoUpdated)?;
    tx.commit()?;
    // hypothesis: playlists containing the track get a new masterPlaylists6.xml Timestamp, as
    // after analysis completion (playlist-create.md §5).
    let playlists: Vec<String> = {
        let mut stmt = lib
            .connection()
            .prepare("SELECT DISTINCT PlaylistID FROM djmdSongPlaylist WHERE rb_local_deleted = 0 AND ContentID = ?1")?;
        stmt.query_map([id.as_str()], |r| r.get(0))?.collect::<mb_masterdb::rusqlite::Result<_>>()?
    };
    if !playlists.is_empty() {
        xml_sync::sync(lib, &playlists)?;
    }
    Ok((Effect::Done, Some(Command::EditTrack { id: id.clone(), edit: edit.with_value(old) })))
}
