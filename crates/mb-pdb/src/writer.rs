//! Writing export.pdb and exportExt.pdb from typed rows. Spec: `docs/re/pdb.md` § Writer.
//!
//! Two ways to build a file, both on top of the storage model in [`crate::engine`]:
//! - [`write_export`] / [`write_ext`] ("compact"): rekordbox's creation sequence (all tables and
//!   the property row in the first transaction, then colours, menu items, categories and sorts in
//!   one transaction each), then every other table in one transaction, in table order. Pages are
//!   dense and no row is ever deleted. This is the layout of a file rekordbox would write if it
//!   inserted the rows table by table.
//! - [`Exporter`]: rekordbox's own operation sequence for an export (per track: lookup rows,
//!   track + property row, image row, track update, playlist entry), including the deleted row
//!   versions the updates leave behind. A fresh export through it reproduces rekordbox's files
//!   page for page; it also continues an existing file ([`Exporter::load`]).
//!
//! IDs are taken from the rows as given; the caller assigns them (rekordbox numbers tracks,
//! artists, albums, genres, labels, keys and images `max + 1` in insertion order).

use std::collections::{BTreeMap, HashMap};

use crate::engine::{Db, RowRef};
use crate::library::{ExportDb, ExtDb};
use crate::page::Pdb;
use crate::schema::DbKind;
use crate::tables::*;
use crate::{Error, Result};

/// Inserts one typed row in the running transaction.
pub fn insert_row<T: TypedRow>(db: &mut Db, row: &T) -> Result<RowRef> {
    db.insert(T::TABLE_TYPE, &row.to_values())
}

fn insert_all<T: TypedRow>(db: &mut Db, rows: &[T]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    db.txn(|db| {
        for r in rows {
            insert_row(db, r)?;
        }
        Ok(())
    })
}

/// export.pdb from typed rows, compact layout (see the module docs).
pub fn write_export(rows: &ExportDb) -> Result<Vec<u8>> {
    let mut db = Db::create(DbKind::Export);
    db.txn(|db| {
        for _ in 0..DbKind::Export.tables().len() {
            db.create_table()?;
        }
        for p in &rows.properties {
            insert_row(db, p)?;
        }
        Ok(())
    })?;
    insert_all(&mut db, &rows.colors)?;
    insert_all(&mut db, &rows.menu_items)?;
    insert_all(&mut db, &rows.categories)?;
    insert_all(&mut db, &rows.sorts)?;
    insert_all(&mut db, &rows.tracks)?;
    insert_all(&mut db, &rows.genres)?;
    insert_all(&mut db, &rows.artists)?;
    insert_all(&mut db, &rows.albums)?;
    insert_all(&mut db, &rows.labels)?;
    insert_all(&mut db, &rows.keys)?;
    insert_all(&mut db, &rows.playlists)?;
    insert_all(&mut db, &rows.playlist_entries)?;
    insert_all(&mut db, &rows.hot_cue_bank_lists)?;
    insert_all(&mut db, &rows.hot_cue_bank_list_entries)?;
    insert_all(&mut db, &rows.histories)?;
    insert_all(&mut db, &rows.history_entries)?;
    insert_all(&mut db, &rows.artwork)?;
    insert_all(&mut db, &rows.mixer_params)?;
    insert_all(&mut db, &rows.hot_cue_bank_points)?;
    db.close();
    Ok(db.to_bytes())
}

/// exportExt.pdb from typed rows, compact layout: the tables and the DBPROPERTYOPTION row in the
/// first transaction (as rekordbox creates the file), then one transaction per table.
pub fn write_ext(rows: &ExtDb) -> Result<Vec<u8>> {
    let mut db = Db::create(DbKind::ExportExt);
    db.txn(|db| {
        for _ in 0..DbKind::ExportExt.tables().len() {
            db.create_table()?;
        }
        for p in &rows.db_property_options {
            insert_row(db, p)?;
        }
        Ok(())
    })?;
    insert_all(&mut db, &rows.tags)?;
    insert_all(&mut db, &rows.recommend_likes)?;
    insert_all(&mut db, &rows.histories)?;
    insert_all(&mut db, &rows.history_entries)?;
    insert_all(&mut db, &rows.tag_tracks)?;
    insert_all(&mut db, &rows.cue_options)?;
    insert_all(&mut db, &rows.content_options)?;
    insert_all(&mut db, &rows.general_options)?;
    db.close();
    Ok(db.to_bytes())
}

/// Live rows of `T` with their places, in chain and slot order.
pub fn locate_rows<T: TypedRow>(pdb: &Pdb) -> Result<Vec<(RowRef, T)>> {
    Ok(read_rows(pdb, T::DB, T::TABLE_TYPE)?
        .iter()
        .map(|r| {
            (
                RowRef {
                    page: r.page,
                    slot: r.slot.index,
                },
                T::from_row(r),
            )
        })
        .collect())
}

/// Lookup tables a track row points into (artist, genre, album, label, key).
#[derive(Debug, Clone, Default)]
pub struct Lookups {
    pub artists: BTreeMap<u32, Artist>,
    pub genres: BTreeMap<u32, Genre>,
    pub albums: BTreeMap<u32, Album>,
    pub labels: BTreeMap<u32, Label>,
    pub keys: BTreeMap<u32, Key>,
    pub artwork: BTreeMap<u32, Artwork>,
}

impl Lookups {
    pub fn from_export(rows: &ExportDb) -> Self {
        Self {
            artists: rows.artists.iter().map(|r| (r.id, r.clone())).collect(),
            genres: rows.genres.iter().map(|r| (r.0.id, r.clone())).collect(),
            albums: rows.albums.iter().map(|r| (r.id, r.clone())).collect(),
            labels: rows.labels.iter().map(|r| (r.0.id, r.clone())).collect(),
            keys: rows.keys.iter().map(|r| (r.id, r.clone())).collect(),
            artwork: rows.artwork.iter().map(|r| (r.id, r.clone())).collect(),
        }
    }
}

fn missing(what: &str, id: u32) -> Error {
    Error::Write(format!("{what} {id} is referenced but not given"))
}

/// rekordbox's export of tracks and playlists into export.pdb, operation by operation
/// (`docs/re/pdb.md` § Writer, § Export operation sequence; `docs/re/export.md` § 1.2, § 1.5).
#[derive(Debug, Clone)]
pub struct Exporter {
    pub db: Db,
    /// Live row of each keyed row: (table type, id) → place.
    rows: HashMap<(u32, u32), RowRef>,
    tracks: HashMap<u32, Track>,
    playlists: HashMap<u32, PlaylistNode>,
    property: Option<(RowRef, Property)>,
    /// Live entry rows per playlist, in insertion order.
    entries: HashMap<u32, Vec<RowRef>>,
}

impl Exporter {
    /// A new stick: the creation transaction (all tables and the property row), then colours,
    /// menu items, categories and sorts, one transaction each (`initPropertyTable` →
    /// `DevInitPropertyTable_`; `docs/re/export.md` § 6.3).
    pub fn create(defaults: &ExportDb) -> Result<Self> {
        let mut db = Db::create(DbKind::Export);
        let property = defaults.properties.first().cloned();
        let prop_ref = db.txn(|db| {
            for _ in 0..DbKind::Export.tables().len() {
                db.create_table()?;
            }
            property.as_ref().map(|p| insert_row(db, p)).transpose()
        })?;
        insert_all(&mut db, &defaults.colors)?;
        insert_all(&mut db, &defaults.menu_items)?;
        insert_all(&mut db, &defaults.categories)?;
        insert_all(&mut db, &defaults.sorts)?;
        Ok(Self {
            db,
            rows: HashMap::new(),
            tracks: HashMap::new(),
            playlists: HashMap::new(),
            property: prop_ref.zip(property),
            entries: HashMap::new(),
        })
    }

    /// Continue an existing export.pdb (incremental mode): the live rows are indexed so later
    /// updates and lookups find them.
    pub fn load(bytes: &[u8]) -> Result<Self> {
        let pdb = Pdb::parse(bytes.to_vec())?;
        let mut rows = HashMap::new();
        let mut tracks = HashMap::new();
        for (r, t) in locate_rows::<Track>(&pdb)? {
            rows.insert((0, t.id), r);
            tracks.insert(t.id, t);
        }
        for (r, x) in locate_rows::<Genre>(&pdb)? {
            rows.insert((1, x.0.id), r);
        }
        for (r, x) in locate_rows::<Artist>(&pdb)? {
            rows.insert((2, x.id), r);
        }
        for (r, x) in locate_rows::<Album>(&pdb)? {
            rows.insert((3, x.id), r);
        }
        for (r, x) in locate_rows::<Label>(&pdb)? {
            rows.insert((4, x.0.id), r);
        }
        for (r, x) in locate_rows::<Key>(&pdb)? {
            rows.insert((5, x.id), r);
        }
        for (r, x) in locate_rows::<Artwork>(&pdb)? {
            rows.insert((13, x.id), r);
        }
        let mut playlists = HashMap::new();
        for (r, x) in locate_rows::<PlaylistNode>(&pdb)? {
            rows.insert((7, x.0.id), r);
            playlists.insert(x.0.id, x);
        }
        let mut entries: HashMap<u32, Vec<RowRef>> = HashMap::new();
        let mut located = locate_rows::<PlaylistEntry>(&pdb)?;
        located.sort_by_key(|(_, e)| (e.playlist_id, e.entry_index));
        for (r, e) in located {
            entries.entry(e.playlist_id).or_default().push(r);
        }
        let property = locate_rows::<Property>(&pdb)?.into_iter().next();
        Ok(Self {
            db: Db::load(DbKind::Export, bytes)?,
            rows,
            tracks,
            playlists,
            property,
            entries,
        })
    }

    /// Ends the current session and starts a new one (rekordbox closes the device database on
    /// eject or quit and reopens it on the next mount; every open and close advances the
    /// transaction id, and each open starts every table on a fresh insert page).
    pub fn reopen(&mut self) {
        self.db.reopen();
    }

    /// Closes the database and returns the file.
    pub fn finish(mut self) -> Vec<u8> {
        self.db.close();
        self.db.to_bytes()
    }

    pub fn track(&self, id: u32) -> Option<&Track> {
        self.tracks.get(&id)
    }

    pub fn has_track(&self, id: u32) -> bool {
        self.tracks.contains_key(&id)
    }

    fn ensure<T: TypedRow>(&mut self, id: u32, row: Option<&T>, what: &str) -> Result<()> {
        if id == 0 || self.rows.contains_key(&(T::TABLE_TYPE, id)) {
            return Ok(());
        }
        let row = row.ok_or_else(|| missing(what, id))?;
        let r = self.db.txn(|db| insert_row(db, row))?;
        self.rows.insert((T::TABLE_TYPE, id), r);
        Ok(())
    }

    /// `getAndAdd*ID` for every lookup a track uses, in `addTrack`'s order (artist, genre,
    /// album, remixer, original artist, composer, label, key); each new row is one transaction.
    pub fn add_lookups(&mut self, t: &Track, l: &Lookups) -> Result<()> {
        self.ensure(t.artist_id, l.artists.get(&t.artist_id), "artist")?;
        self.ensure(t.genre_id, l.genres.get(&t.genre_id), "genre")?;
        self.ensure(t.album_id, l.albums.get(&t.album_id), "album")?;
        self.ensure(t.remixer_id, l.artists.get(&t.remixer_id), "artist")?;
        self.ensure(
            t.original_artist_id,
            l.artists.get(&t.original_artist_id),
            "artist",
        )?;
        self.ensure(t.composer_id, l.artists.get(&t.composer_id), "artist")?;
        self.ensure(t.label_id, l.labels.get(&t.label_id), "label")?;
        self.ensure(t.key_id, l.keys.get(&t.key_id), "key")?;
        Ok(())
    }

    /// Replaces the property row, as `DevAddContent_` does after each track (copying UPDATE:
    /// new row, then the old one deleted). Runs inside the caller's transaction.
    fn update_property(
        db: &mut Db,
        prop: &mut Option<(RowRef, Property)>,
        count: u32,
    ) -> Result<()> {
        if let Some((r, p)) = prop.as_mut() {
            p.track_count = count;
            *r = db.update(Property::TABLE_TYPE, *r, &p.to_values())?;
        }
        Ok(())
    }

    /// Exports one new track the way `execExport` does: lookups, then the track row with
    /// IMAGEID 0 together with the property row (one `DevAddContent_` transaction), then — when
    /// the track has artwork — the image row and the track row update that sets IMAGEID.
    /// Playlist entries are added separately ([`Exporter::add_entry`]).
    pub fn add_track(&mut self, t: &Track, l: &Lookups) -> Result<()> {
        if self.tracks.contains_key(&t.id) {
            return Err(Error::Write(format!(
                "track {} is already on the device",
                t.id
            )));
        }
        self.add_lookups(t, l)?;
        let mut first = t.clone();
        first.artwork_id = 0;
        let count = self.tracks.len() as u32 + 1;
        let mut prop = self.property.take();
        let r = self.db.txn(|db| {
            let r = insert_row(db, &first)?;
            Self::update_property(db, &mut prop, count)?;
            Ok(r)
        });
        self.property = prop;
        let mut r = r?;
        if t.artwork_id != 0 {
            let art = l
                .artwork
                .get(&t.artwork_id)
                .ok_or_else(|| missing("image", t.artwork_id))?;
            let ar = self.db.txn(|db| insert_row(db, art))?;
            self.rows.insert((Artwork::TABLE_TYPE, art.id), ar);
            r = self
                .db
                .txn(|db| db.update(Track::TABLE_TYPE, r, &t.to_values()))?;
        }
        self.rows.insert((Track::TABLE_TYPE, t.id), r);
        self.tracks.insert(t.id, t.clone());
        Ok(())
    }

    /// Rewrites a track row with new values (copying UPDATE: new row, old one deleted).
    pub fn update_track(&mut self, t: &Track) -> Result<()> {
        let old = *self
            .rows
            .get(&(Track::TABLE_TYPE, t.id))
            .ok_or_else(|| missing("track", t.id))?;
        let r = self
            .db
            .txn(|db| db.update(Track::TABLE_TYPE, old, &t.to_values()))?;
        self.rows.insert((Track::TABLE_TYPE, t.id), r);
        self.tracks.insert(t.id, t.clone());
        Ok(())
    }

    /// A playlist (or folder) row in its own transaction.
    pub fn insert_playlist(&mut self, node: &PlaylistNode) -> Result<()> {
        let r = self.db.txn(|db| insert_row(db, node))?;
        self.rows.insert((PlaylistNode::TABLE_TYPE, node.0.id), r);
        self.playlists.insert(node.0.id, node.clone());
        Ok(())
    }

    /// The first playlist of a new stick: one transaction that inserts the row and rewrites it
    /// with the same values (copying UPDATE), leaving a deleted copy in slot 0 — what the oracle
    /// stick shows for its first list.
    pub fn create_first_playlist(&mut self, node: &PlaylistNode) -> Result<()> {
        let values = node.to_values();
        let r = self.db.txn(|db| {
            let r = db.insert(PlaylistNode::TABLE_TYPE, &values)?;
            db.update(PlaylistNode::TABLE_TYPE, r, &values)
        })?;
        self.rows.insert((PlaylistNode::TABLE_TYPE, node.0.id), r);
        self.playlists.insert(node.0.id, node.clone());
        Ok(())
    }

    /// Rewrites a playlist row in place with the same values (a write transaction whose only
    /// trace is the page's transaction id).
    pub fn touch_playlist(&mut self, id: u32) -> Result<()> {
        let node = self
            .playlists
            .get(&id)
            .cloned()
            .ok_or_else(|| missing("playlist", id))?;
        let r = self
            .row_ref(PlaylistNode::TABLE_TYPE, id)
            .ok_or_else(|| missing("playlist", id))?;
        let values = node.to_values();
        self.db
            .txn(|db| db.update_in_place(PlaylistNode::TABLE_TYPE, r, &values))
    }

    /// Rewrites playlist rows in one transaction, in the given order (copying UPDATE per row).
    pub fn update_playlists(&mut self, nodes: &[PlaylistNode]) -> Result<()> {
        let mut refs = Vec::new();
        for n in nodes {
            refs.push(
                *self
                    .rows
                    .get(&(PlaylistNode::TABLE_TYPE, n.0.id))
                    .ok_or_else(|| missing("playlist", n.0.id))?,
            );
        }
        let new = self.db.txn(|db| {
            nodes
                .iter()
                .zip(&refs)
                .map(|(n, r)| db.update(PlaylistNode::TABLE_TYPE, *r, &n.to_values()))
                .collect::<Result<Vec<_>>>()
        })?;
        for (n, r) in nodes.iter().zip(new) {
            self.rows.insert((PlaylistNode::TABLE_TYPE, n.0.id), r);
            self.playlists.insert(n.0.id, n.clone());
        }
        Ok(())
    }

    /// Creates a top-level playlist the way the oracle export did: the row is inserted with the
    /// next free position, then the new list is moved to position 0 and its siblings move down
    /// by one (one transaction rewriting the siblings in position order, then the new list).
    pub fn create_playlist_at_top(&mut self, node: &PlaylistNode) -> Result<()> {
        let mut siblings: Vec<PlaylistNode> = self
            .playlists
            .values()
            .filter(|p| p.0.parent_id == node.0.parent_id)
            .cloned()
            .collect();
        siblings.sort_by_key(|p| p.0.seq);
        let mut first = node.clone();
        first.0.seq = siblings.len() as u32;
        self.insert_playlist(&first)?;
        let mut moved: Vec<PlaylistNode> = siblings
            .into_iter()
            .map(|mut p| {
                p.0.seq += 1;
                p
            })
            .collect();
        let mut top = node.clone();
        top.0.seq = 0;
        moved.push(top);
        self.update_playlists(&moved)
    }

    /// Appends a track to a playlist (`add_track_to_device_playlist`): one row, one transaction.
    pub fn add_entry(&mut self, e: &PlaylistEntry) -> Result<RowRef> {
        let r = self.db.txn(|db| insert_row(db, e))?;
        self.entries.entry(e.playlist_id).or_default().push(r);
        Ok(r)
    }

    /// Number of entries a playlist has.
    pub fn entry_count(&self, playlist: u32) -> u32 {
        self.entries.get(&playlist).map_or(0, |v| v.len() as u32)
    }

    /// Deletes every entry of a playlist in one transaction (`removeAllTracksFromPlaylist`).
    pub fn clear_playlist(&mut self, playlist: u32) -> Result<()> {
        let refs = self.entries.remove(&playlist).unwrap_or_default();
        if refs.is_empty() {
            return Ok(());
        }
        self.db.txn(|db| {
            for r in &refs {
                db.delete(PlaylistEntry::TABLE_TYPE, *r)?;
            }
            Ok(())
        })
    }

    /// Place of the live row of a keyed table (tracks, lookups, images, playlists) by ID.
    pub fn row_ref(&self, table_type: u32, id: u32) -> Option<RowRef> {
        self.rows.get(&(table_type, id)).copied()
    }

    pub fn playlist(&self, id: u32) -> Option<&PlaylistNode> {
        self.playlists.get(&id)
    }
}

/// Playlist tree in insertion order: parents before children, siblings by SEQ.
fn tree_order(nodes: &[PlaylistNode]) -> Vec<PlaylistNode> {
    let mut by_parent: BTreeMap<u32, Vec<&PlaylistNode>> = BTreeMap::new();
    for n in nodes {
        by_parent.entry(n.0.parent_id).or_default().push(n);
    }
    for v in by_parent.values_mut() {
        v.sort_by_key(|n| (n.0.seq, n.0.id));
    }
    let mut out = Vec::new();
    let mut stack = vec![0u32];
    let mut seen = std::collections::HashSet::new();
    while let Some(parent) = stack.pop() {
        if let Some(children) = by_parent.get(&parent) {
            for n in children.iter().rev() {
                if seen.insert(n.0.id) {
                    stack.push(n.0.id);
                }
            }
            for n in children {
                out.push((*n).clone());
            }
        }
    }
    // Nodes whose parent is missing keep their place at the end.
    for n in nodes {
        if !out.iter().any(|o| o.0.id == n.0.id) {
            out.push(n.clone());
        }
    }
    out
}

/// export.pdb from typed rows in one rekordbox-style export session (fresh stick):
/// creation, playlist rows, then every playlist's tracks in entry order (lookups, track +
/// property, image, track update, entry), then tracks in no playlist, then the remaining
/// tables compactly. The first top-level list is created the way the oracle export created its
/// first list; further lists are inserted with their final position (**hypothesis**: rekordbox
/// moves a new list to position 0 later in the session, see `docs/re/pdb.md`).
pub fn export_like_rekordbox(rows: &ExportDb) -> Result<Vec<u8>> {
    let mut defaults = ExportDb {
        colors: rows.colors.clone(),
        menu_items: rows.menu_items.clone(),
        categories: rows.categories.clone(),
        sorts: rows.sorts.clone(),
        ..Default::default()
    };
    if let Some(p) = rows.properties.first() {
        let mut p = p.clone();
        p.track_count = 0;
        defaults.properties.push(p);
    }
    let lookups = Lookups::from_export(rows);
    let tracks: BTreeMap<u32, &Track> = rows.tracks.iter().map(|t| (t.id, t)).collect();
    let mut ex = Exporter::create(&defaults)?;
    for (i, node) in tree_order(&rows.playlists).iter().enumerate() {
        if i == 0 && node.0.parent_id == 0 && node.0.seq == 0 {
            ex.create_first_playlist(node)?;
        } else {
            ex.insert_playlist(node)?;
        }
    }
    let mut entries: BTreeMap<u32, Vec<PlaylistEntry>> = BTreeMap::new();
    for e in &rows.playlist_entries {
        entries.entry(e.playlist_id).or_default().push(*e);
    }
    for node in tree_order(&rows.playlists) {
        let Some(list) = entries.get_mut(&node.0.id) else {
            continue;
        };
        list.sort_by_key(|e| e.entry_index);
        for e in list.iter() {
            if !ex.has_track(e.track_id)
                && let Some(t) = tracks.get(&e.track_id)
            {
                ex.add_track(t, &lookups)?;
            }
            ex.add_entry(e)?;
        }
    }
    for t in &rows.tracks {
        if !ex.has_track(t.id) {
            ex.add_track(t, &lookups)?;
        }
    }
    // Entries of playlists that are not in the tree (dangling) keep their rows.
    for (pl, list) in &entries {
        if !rows.playlists.iter().any(|p| p.0.id == *pl) {
            for e in list {
                ex.add_entry(e)?;
            }
        }
    }
    // Lookup rows no track uses.
    let unused = |t: u32, id: u32, ex: &Exporter| ex.row_ref(t, id).is_none();
    let rest = ExportDb {
        genres: rows
            .genres
            .iter()
            .filter(|r| unused(1, r.0.id, &ex))
            .cloned()
            .collect(),
        artists: rows
            .artists
            .iter()
            .filter(|r| unused(2, r.id, &ex))
            .cloned()
            .collect(),
        albums: rows
            .albums
            .iter()
            .filter(|r| unused(3, r.id, &ex))
            .cloned()
            .collect(),
        labels: rows
            .labels
            .iter()
            .filter(|r| unused(4, r.0.id, &ex))
            .cloned()
            .collect(),
        keys: rows
            .keys
            .iter()
            .filter(|r| unused(5, r.id, &ex))
            .cloned()
            .collect(),
        artwork: rows
            .artwork
            .iter()
            .filter(|r| unused(13, r.id, &ex))
            .cloned()
            .collect(),
        ..Default::default()
    };
    let db = &mut ex.db;
    insert_all(db, &rest.genres)?;
    insert_all(db, &rest.artists)?;
    insert_all(db, &rest.albums)?;
    insert_all(db, &rest.labels)?;
    insert_all(db, &rest.keys)?;
    insert_all(db, &rest.artwork)?;
    insert_all(db, &rows.hot_cue_bank_lists)?;
    insert_all(db, &rows.hot_cue_bank_list_entries)?;
    insert_all(db, &rows.histories)?;
    insert_all(db, &rows.history_entries)?;
    insert_all(db, &rows.mixer_params)?;
    insert_all(db, &rows.hot_cue_bank_points)?;
    Ok(ex.finish())
}

/// exportExt.pdb as rekordbox writes it for a fresh stick: the tables and the DBPROPERTYOPTION
/// row in the first transaction, the My Tag tree in one transaction (each column followed by its
/// tags, by SEQ), then the other tables.
pub fn ext_like_rekordbox(rows: &ExtDb) -> Result<Vec<u8>> {
    let mut sorted = rows.clone();
    sorted.tags = my_tag_tree_order(&rows.tags);
    write_ext(&sorted)
}

/// My Tag rows in the order rekordbox inserts them: each column (by SEQ) followed by its tags
/// (by SEQ) (`docs/re/ops/export-devlib.md` Phase 4; verified on the oracle stick).
pub fn my_tag_tree_order(tags: &[MyTag]) -> Vec<MyTag> {
    let col_seq: HashMap<u32, u32> = tags
        .iter()
        .filter(|t| t.parent_id == 0)
        .map(|t| (t.id, t.seq))
        .collect();
    let mut out = tags.to_vec();
    out.sort_by_key(|t| {
        if t.parent_id == 0 {
            (t.seq, false, 0)
        } else {
            (
                col_seq.get(&t.parent_id).copied().unwrap_or(u32::MAX),
                true,
                t.seq,
            )
        }
    });
    out
}
