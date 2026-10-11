//! Playlist ops. Specs: `docs/re/ops/playlist-create.md` (create, move-to-top, cloud filter row,
//! rename; verified), `docs/re/ops/playlist-add-tracks.md` (append; verified). Folders, delete,
//! move, insert at a position, entry removal and reordering were not exercised in the trace; they
//! are built from the traced building blocks and rekordbox's statement templates found in the
//! binary, and every such place is marked `hypothesis`.
//!
//! Smart playlists (`Attribute` 4) are read-only: every op that targets one directly is refused.

use crate::lookup::{any_row_with_id, fresh_id};
use crate::{Command, DbId, Effect, Error, Library, Result, xml_sync};
use mb_masterdb::WriteTxn;
use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::rusqlite::{Connection, OptionalExtension, params_from_iter};
use mb_masterdb::write::new_uuid;
use std::collections::HashMap;

pub const ATTRIBUTE_PLAYLIST: i64 = 0;
pub const ATTRIBUTE_FOLDER: i64 = 1;
pub const ATTRIBUTE_SMART: i64 = 4;
/// `djmdPlaylist.ParentID` of top-level nodes.
pub const ROOT: &str = "root";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Playlist,
    Folder,
}

impl NodeKind {
    pub fn attribute(self) -> i64 {
        match self {
            Self::Playlist => ATTRIBUTE_PLAYLIST,
            Self::Folder => ATTRIBUTE_FOLDER,
        }
    }
}

/// A `djmdPlaylist` row (the columns musicbox keeps for restore).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRow {
    pub id: String,
    /// `"root"` or the parent's ID.
    pub parent: String,
    pub seq: i64,
    pub name: String,
    pub attribute: i64,
    pub uuid: String,
    pub image_path: Option<String>,
    pub smart_list: Option<String>,
}

/// A `djmdSongPlaylist` row.
#[derive(Debug, Clone, PartialEq)]
pub struct EntrySnapshot {
    pub id: String,
    pub uuid: String,
    pub content_id: DbId,
    pub track_no: i64,
}

/// A `djmdCloudFilterPlaylist` row.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudFilterSnapshot {
    pub id: String,
    pub playlist_uuid: String,
    pub uuid: String,
    pub seq: Option<i64>,
    pub parent_id: Option<String>,
}

/// A deleted subtree, enough to re-create it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistSnapshot {
    /// Pre-order; the deleted node first, with its entries in `TrackNo` order.
    pub nodes: Vec<(NodeRow, Vec<EntrySnapshot>)>,
    pub cloud_filters: Vec<CloudFilterSnapshot>,
    /// 1-based position of the deleted node among its siblings.
    pub position: u32,
}

const NODE_COLUMNS: &str = "ID, ParentID, Seq, Name, Attribute, UUID, ImagePath, SmartList";

fn node_from_row(r: &mb_masterdb::rusqlite::Row<'_>) -> mb_masterdb::rusqlite::Result<NodeRow> {
    Ok(NodeRow {
        id: r.get(0)?,
        parent: r.get::<_, Option<String>>(1)?.unwrap_or_else(|| ROOT.into()),
        seq: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
        name: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        attribute: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
        uuid: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        image_path: r.get(6)?,
        smart_list: r.get(7)?,
    })
}

/// The live node with this ID.
pub fn node(conn: &Connection, id: &str) -> Result<Option<NodeRow>> {
    Ok(conn
        .query_row(&format!("SELECT {NODE_COLUMNS} FROM djmdPlaylist WHERE ID = ?1 AND rb_local_deleted = 0"), [id], node_from_row)
        .optional()?)
}

fn require_node(conn: &Connection, id: &str) -> Result<NodeRow> {
    node(conn, id)?.ok_or_else(|| Error::NotFound { what: "playlist", id: id.to_owned() })
}

/// Live children of `parent` in `Seq` order, as rekordbox's ORM reads them
/// (`SELECT * FROM djmdPlaylist WHERE ParentID = '<p>' AND ID != '<id>' AND rb_local_deleted = 0 ORDER BY Seq ASC`).
pub fn children(conn: &Connection, parent: &str, exclude: Option<&str>) -> Result<Vec<NodeRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {NODE_COLUMNS} FROM djmdPlaylist WHERE ParentID = ?1 AND ID != ?2 AND rb_local_deleted = 0 ORDER BY Seq ASC, rowid"
    ))?;
    Ok(stmt.query_map([parent, exclude.unwrap_or("")], node_from_row)?.collect::<mb_masterdb::rusqlite::Result<_>>()?)
}

/// Live entries of a playlist in `TrackNo` order.
pub fn entries(conn: &Connection, playlist: &str) -> Result<Vec<EntrySnapshot>> {
    let mut stmt = conn.prepare(
        "SELECT ID, UUID, ContentID, TrackNo FROM djmdSongPlaylist WHERE PlaylistID = ?1 AND rb_local_deleted = 0 ORDER BY TrackNo, rowid",
    )?;
    Ok(stmt
        .query_map([playlist], |r| {
            Ok(EntrySnapshot {
                id: r.get(0)?,
                uuid: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                content_id: DbId(r.get(2)?),
                track_no: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            })
        })?
        .collect::<mb_masterdb::rusqlite::Result<_>>()?)
}

/// `None` → root; otherwise the ID of a live folder.
fn resolve_parent(conn: &Connection, parent: Option<&DbId>) -> Result<String> {
    let Some(p) = parent else { return Ok(ROOT.to_owned()) };
    let n = require_node(conn, p.as_str())?;
    if n.attribute != ATTRIBUTE_FOLDER {
        return Err(Error::Invalid(format!("{} is not a folder", p)));
    }
    Ok(n.id)
}

fn refuse_smart(n: &NodeRow) -> Result<()> {
    if n.attribute == ATTRIBUTE_SMART {
        return Err(Error::SmartPlaylistReadOnly(n.id.clone()));
    }
    Ok(())
}

/// A playlist that can hold tracks (not a folder, not a smart list).
fn require_plain_playlist(conn: &Connection, id: &str) -> Result<NodeRow> {
    let n = require_node(conn, id)?;
    refuse_smart(&n)?;
    if n.attribute != ATTRIBUTE_PLAYLIST {
        return Err(Error::Invalid(format!("{id} is a folder")));
    }
    Ok(n)
}

/// ORM update of a tree node (playlist-create.md §2):
/// `UPDATE djmdPlaylist SET Attribute, Name, ParentID, Seq, rb_*…, rb_local_usn, updated_at WHERE ID`.
fn orm_write(tx: &WriteTxn<'_>, n: &NodeRow) -> Result<i64> {
    Ok(tx.update_orm(
        "djmdPlaylist",
        &n.id,
        &[
            ("Attribute", Value::Integer(n.attribute)),
            ("Name", Value::Text(n.name.clone())),
            ("ParentID", Value::Text(n.parent.clone())),
            ("Seq", Value::Integer(n.seq)),
        ],
    )?)
}

/// rekordbox's ORM move routine (traced for the move-to-top after create, and for the sampler
/// nodes at startup): the node is written with its new parent and `Seq`, then every sibling in
/// the new order whose `Seq` changes, one ORM update (one USN) each. Siblings already in place
/// are not written.
fn place(tx: &WriteTxn<'_>, n: &mut NodeRow, parent: &str, position: u32) -> Result<()> {
    let old_parent = n.parent.clone();
    let siblings = children(tx.connection(), parent, Some(&n.id))?;
    let pos = (position.max(1) as usize).min(siblings.len() + 1);
    n.parent = parent.to_owned();
    n.seq = pos as i64;
    // hypothesis: the moved node is written even when its Seq and parent do not change (it
    // changed in every traced move).
    orm_write(tx, n)?;
    let mut seq = 1i64;
    for mut s in siblings {
        if seq == pos as i64 {
            seq += 1;
        }
        if s.seq != seq {
            s.seq = seq;
            orm_write(tx, &s)?;
        }
        seq += 1;
    }
    if old_parent != parent {
        // hypothesis: the old parent's remaining children close the gap the same way.
        close_seq_gap(tx, &old_parent)?;
    }
    Ok(())
}

/// Renumbers the children of `parent` to 1…n with ORM updates for the rows that change.
// hypothesis: rekordbox closes `Seq` gaps after a delete or a move-out (the binary has
// `update __TABLE_NAME__ set __SEQ_COLUMN__ = __SEQ_COLUMN__ - 1 where __SEQ_COLUMN__ > %d and
// __PARENT_ID_COLUMN__ = %d`, without USN columns, so probably for the device library); the ORM
// routine above is the traced master.db path and is used here.
fn close_seq_gap(tx: &WriteTxn<'_>, parent: &str) -> Result<()> {
    for (i, mut s) in children(tx.connection(), parent, None)?.into_iter().enumerate() {
        if s.seq != i as i64 + 1 {
            s.seq = i as i64 + 1;
            orm_write(tx, &s)?;
        }
    }
    Ok(())
}

fn position_among_siblings(conn: &Connection, n: &NodeRow) -> Result<u32> {
    let siblings = children(conn, &n.parent, None)?;
    Ok(siblings.iter().position(|s| s.id == n.id).map_or(1, |i| i as u32 + 1))
}

// ---------------------------------------------------------------------------------------------
// Create / rename / move / delete / restore

/// playlist-create.md: insert at `max(Seq) + 1` (ORM, one USN), move to the top (one USN per
/// rewritten node), then the `djmdCloudFilterPlaylist` row (one USN). rekordbox runs these in
/// autocommit mode; musicbox wraps them in one transaction (same stored result).
pub(crate) fn create(lib: &mut Library, parent: Option<&DbId>, name: &str, kind: NodeKind) -> Result<(Effect, Option<Command>)> {
    // hypothesis: folders (Attribute 1) and sub-folder children are created by the same
    // statements as a top-level playlist, with ParentID = the folder's ID.
    let parent_key = resolve_parent(lib.connection(), parent)?;
    let tx = lib.writer.transaction()?;
    let max_seq: Option<i64> = tx.connection().query_row(
        "SELECT max(Seq) AS maxSeq FROM djmdPlaylist WHERE rb_local_deleted = 0 AND ParentID = ?1",
        [&parent_key],
        |r| r.get(0),
    )?;
    // ORM inserts check the new ID first (`SELECT * FROM djmdPlaylist WHERE ID = '<new>' AND
    // rb_local_deleted = 0`); a soft-deleted row would still block the primary key, so both count.
    let id = fresh_id(&tx, "djmdPlaylist")?;
    let mut node = NodeRow {
        id: id.clone(),
        parent: parent_key.clone(),
        seq: max_seq.unwrap_or(0) + 1,
        name: name.to_owned(),
        attribute: kind.attribute(),
        uuid: new_uuid(),
        image_path: None,
        smart_list: None,
    };
    tx.insert(
        "djmdPlaylist",
        &[
            ("Attribute", Value::Integer(node.attribute)),
            ("ID", Value::Text(node.id.clone())),
            ("Name", Value::Text(node.name.clone())),
            ("ParentID", Value::Text(node.parent.clone())),
            ("Seq", Value::Integer(node.seq)),
            ("UUID", Value::Text(node.uuid.clone())),
        ],
    )?;
    // hypothesis: "top" is the first position of the parent regardless of the tree selection
    // (nothing was selected in the trace).
    place(&tx, &mut node, &parent_key, 1)?;
    // hypothesis: folders get a cloud filter row too (only playlists were traced).
    let filter_id = fresh_id(&tx, "djmdCloudFilterPlaylist")?;
    tx.insert(
        "djmdCloudFilterPlaylist",
        &[
            ("ID", Value::Text(filter_id)),
            ("PlaylistUUID", Value::Text(node.uuid.clone())),
            ("UUID", Value::Text(new_uuid())),
        ],
    )?;
    tx.commit()?;
    xml_sync::sync(lib, std::slice::from_ref(&id))?;
    Ok((Effect::PlaylistCreated { id: DbId(id.clone()) }, Some(Command::DeletePlaylist { id: DbId(id) })))
}

/// playlist-create.md §4: one builder update of `Name` (one USN); the cloud filter row is not
/// touched.
pub(crate) fn rename(lib: &mut Library, id: &DbId, name: &str) -> Result<(Effect, Option<Command>)> {
    let n = require_node(lib.connection(), id.as_str())?;
    refuse_smart(&n)?;
    if n.name == name {
        return Ok((Effect::Unchanged, None));
    }
    let tx = lib.writer.transaction()?;
    tx.update("djmdPlaylist", "ID", Value::Text(n.id.clone()), &[("Name", Value::Text(name.to_owned()))])?;
    tx.commit()?;
    // hypothesis: a rename counts as a change of the list for the masterPlaylists6.xml Timestamp.
    xml_sync::sync(lib, std::slice::from_ref(&n.id))?;
    Ok((Effect::Done, Some(Command::RenamePlaylist { id: id.clone(), name: n.name })))
}

/// hypothesis (not traced): a tree move uses the same ORM move routine as the traced
/// move-to-top after create.
pub(crate) fn move_node(lib: &mut Library, id: &DbId, parent: Option<&DbId>, position: u32) -> Result<(Effect, Option<Command>)> {
    let mut n = require_node(lib.connection(), id.as_str())?;
    refuse_smart(&n)?;
    let target = resolve_parent(lib.connection(), parent)?;
    // A node cannot move into its own subtree.
    let mut cursor = target.clone();
    while cursor != ROOT {
        if cursor == n.id {
            return Err(Error::Invalid(format!("cannot move {} into its own subtree", n.id)));
        }
        cursor = node(lib.connection(), &cursor)?.map_or_else(|| ROOT.to_owned(), |p| p.parent);
    }
    let old_parent = n.parent.clone();
    let old_position = position_among_siblings(lib.connection(), &n)?;
    let new_count = children(lib.connection(), &target, Some(&n.id))?.len() as u32 + 1;
    let new_position = position.clamp(1, new_count);
    if old_parent == target && old_position == new_position {
        return Ok((Effect::Unchanged, None));
    }
    let tx = lib.writer.transaction()?;
    place(&tx, &mut n, &target, new_position)?;
    tx.commit()?;
    // A pure Seq shift keeps the timestamps (verified for siblings in oracle-a).
    xml_sync::sync(lib, &[])?;
    let old_parent = (old_parent != ROOT).then_some(DbId(old_parent));
    Ok((Effect::Done, Some(Command::MovePlaylist { id: id.clone(), parent: old_parent, position: old_position })))
}

/// hypothesis (not traced): DELETE-ROWS over the subtree's entries, cloud filter rows and
/// playlist rows (one USN each, in that order), then the remaining siblings close the `Seq` gap
/// (one ORM update per moved sibling). The binary's generic templates
/// `delete from __TABLE__ where … and ID in (__IDs__)` / soft-delete counterpart are the
/// DELETE-ROWS macro of the traced track removal.
pub(crate) fn delete(lib: &mut Library, id: &DbId) -> Result<(Effect, Option<Command>)> {
    let root = require_node(lib.connection(), id.as_str())?;
    refuse_smart(&root)?;
    let position = position_among_siblings(lib.connection(), &root)?;
    // Pre-order subtree with entries.
    let mut nodes: Vec<(NodeRow, Vec<EntrySnapshot>)> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(n) = stack.pop() {
        let e = entries(lib.connection(), &n.id)?;
        let kids = children(lib.connection(), &n.id, None)?;
        nodes.push((n, e));
        stack.extend(kids.into_iter().rev());
    }
    let ids: Vec<Value> = nodes.iter().map(|(n, _)| Value::Text(n.id.clone())).collect();
    let uuids: Vec<Value> = nodes.iter().map(|(n, _)| Value::Text(n.uuid.clone())).collect();
    let cloud_filters = {
        let list = vec!["?"; uuids.len()].join(", ");
        let mut stmt = lib.connection().prepare(&format!(
            "SELECT ID, PlaylistUUID, UUID, Seq, ParentID FROM djmdCloudFilterPlaylist WHERE rb_local_deleted = 0 AND PlaylistUUID IN ({list})"
        ))?;
        stmt.query_map(params_from_iter(uuids.iter()), |r| {
            Ok(CloudFilterSnapshot {
                id: r.get(0)?,
                playlist_uuid: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                uuid: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                seq: r.get(3)?,
                parent_id: r.get(4)?,
            })
        })?
        .collect::<mb_masterdb::rusqlite::Result<Vec<_>>>()?
    };
    let tx = lib.writer.transaction()?;
    tx.delete_rows("djmdSongPlaylist", "PlaylistID", &ids)?;
    tx.delete_rows("djmdCloudFilterPlaylist", "PlaylistUUID", &uuids)?;
    tx.delete_rows("djmdPlaylist", "ID", &ids)?;
    close_seq_gap(&tx, &root.parent)?;
    tx.commit()?;
    xml_sync::sync(lib, &[])?;
    let snapshot = PlaylistSnapshot { nodes, cloud_filters, position };
    Ok((Effect::Done, Some(Command::RestorePlaylist { snapshot })))
}

/// Inverse of [`delete`]: re-inserts the subtree with its original IDs and UUIDs where they are
/// free (a never-synced row was hard-deleted, so they normally are), puts the root back at its
/// old position (ORM move routine) and re-adds the entries, one batch (one USN) per playlist.
pub(crate) fn restore(lib: &mut Library, snapshot: &PlaylistSnapshot) -> Result<(Effect, Option<Command>)> {
    let Some((root, _)) = snapshot.nodes.first() else { return Ok((Effect::Unchanged, None)) };
    if root.parent != ROOT {
        resolve_parent(lib.connection(), Some(&DbId(root.parent.clone())))?;
    }
    let tx = lib.writer.transaction()?;
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut restored: Vec<String> = Vec::new();
    for (i, (n, _)) in snapshot.nodes.iter().enumerate() {
        let new_id = if any_row_with_id(&tx, "djmdPlaylist", &n.id)? { fresh_id(&tx, "djmdPlaylist")? } else { n.id.clone() };
        let parent = if i == 0 { n.parent.clone() } else { id_map.get(&n.parent).cloned().unwrap_or_else(|| n.parent.clone()) };
        let seq = if i == 0 {
            let max: Option<i64> = tx.connection().query_row(
                "SELECT max(Seq) FROM djmdPlaylist WHERE rb_local_deleted = 0 AND ParentID = ?1",
                [&parent],
                |r| r.get(0),
            )?;
            max.unwrap_or(0) + 1
        } else {
            n.seq
        };
        let mut cols = vec![
            ("Attribute", Value::Integer(n.attribute)),
            ("ID", Value::Text(new_id.clone())),
            ("Name", Value::Text(n.name.clone())),
            ("ParentID", Value::Text(parent.clone())),
            ("Seq", Value::Integer(seq)),
            ("UUID", Value::Text(n.uuid.clone())),
        ];
        if let Some(p) = &n.image_path {
            cols.push(("ImagePath", Value::Text(p.clone())));
        }
        if let Some(s) = &n.smart_list {
            cols.push(("SmartList", Value::Text(s.clone())));
        }
        tx.insert("djmdPlaylist", &cols)?;
        if i == 0 {
            let mut placed = NodeRow { id: new_id.clone(), parent: parent.clone(), seq, ..n.clone() };
            place(&tx, &mut placed, &parent, snapshot.position)?;
        }
        id_map.insert(n.id.clone(), new_id.clone());
        restored.push(new_id);
    }
    for cf in &snapshot.cloud_filters {
        let id = if any_row_with_id(&tx, "djmdCloudFilterPlaylist", &cf.id)? { fresh_id(&tx, "djmdCloudFilterPlaylist")? } else { cf.id.clone() };
        let mut cols = vec![
            ("ID", Value::Text(id)),
            ("PlaylistUUID", Value::Text(cf.playlist_uuid.clone())),
            ("UUID", Value::Text(cf.uuid.clone())),
        ];
        if let Some(s) = cf.seq {
            cols.push(("Seq", Value::Integer(s)));
        }
        if let Some(p) = &cf.parent_id {
            cols.push(("ParentID", Value::Text(p.clone())));
        }
        tx.insert("djmdCloudFilterPlaylist", &cols)?;
    }
    for (n, list) in &snapshot.nodes {
        let live: Vec<&EntrySnapshot> = list.iter().filter(|e| content_is_live(tx.connection(), e.content_id.as_str())).collect();
        if live.is_empty() {
            continue;
        }
        let playlist_id = &id_map[&n.id];
        let mut rows = Vec::new();
        for (i, e) in live.iter().enumerate() {
            let entry_id = if any_row_with_id(&tx, "djmdSongPlaylist", &e.id)? { new_uuid() } else { e.id.clone() };
            rows.push(entry_columns(&entry_id, playlist_id, e.content_id.as_str(), i as i64 + 1, &e.uuid));
        }
        tx.insert_batch("djmdSongPlaylist", &rows)?;
    }
    tx.commit()?;
    xml_sync::sync(lib, &restored)?;
    let root_id = DbId(restored[0].clone());
    Ok((Effect::PlaylistsRestored { ids: restored.into_iter().map(DbId).collect() }, Some(Command::DeletePlaylist { id: root_id })))
}

// ---------------------------------------------------------------------------------------------
// Entries

fn content_is_live(conn: &Connection, id: &str) -> bool {
    conn.query_row("SELECT 1 FROM djmdContent WHERE ID = ?1 AND rb_local_deleted = 0", [id], |_| Ok(())).is_ok()
}

/// Columns of the traced `djmdSongPlaylist` insert (playlist-add-tracks.md):
/// `(ID, PlaylistID, ContentID, TrackNo, UUID, rb_*…)`; `ID` and `UUID` are two v4 UUIDs.
fn entry_columns(id: &str, playlist: &str, content: &str, track_no: i64, uuid: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("ID", Value::Text(id.to_owned())),
        ("PlaylistID", Value::Text(playlist.to_owned())),
        ("ContentID", Value::Text(content.to_owned())),
        ("TrackNo", Value::Integer(track_no)),
        ("UUID", Value::Text(uuid.to_owned())),
    ]
}

fn live_entry_count(conn: &Connection, playlist: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT count(ID) AS numSongs FROM djmdSongPlaylist WHERE rb_local_deleted = 0 AND PlaylistID = ?1",
        [playlist],
        |r| r.get(0),
    )?)
}

/// playlist-add-tracks.md: `<n0>` = live entry count, then **one** USN for the whole batch and
/// one insert per track with `TrackNo` n0+1…, every row carrying the same `<U>` (rekordbox
/// bumps before `begin exclusive`; the stored result is the same). The playlist row itself is
/// not updated.
pub(crate) fn add_tracks(lib: &mut Library, playlist: &DbId, content_ids: &[DbId], position: Option<u32>) -> Result<(Effect, Option<Command>)> {
    let p = require_plain_playlist(lib.connection(), playlist.as_str())?;
    if content_ids.is_empty() {
        return Ok((Effect::Unchanged, None));
    }
    for c in content_ids {
        if !content_is_live(lib.connection(), c.as_str()) {
            return Err(Error::NotFound { what: "track", id: c.0.clone() });
        }
    }
    // hypothesis: duplicates are allowed (no duplicate check was traced; rekordbox reads the
    // existing ContentIDs first, probably for its "already in playlist" prompt).
    let n0 = live_entry_count(lib.connection(), &p.id)?;
    let tx = lib.writer.transaction()?;
    let start = match position {
        Some(pos) if (pos as i64) <= n0 => {
            let pos = pos.max(1) as i64;
            // hypothesis: inserting before an existing entry first shifts the tail (builder
            // update, one USN; the binary's range template is `__COLUMN__ = __COLUMN__ + 1 …
            // where __COLUMN__ >= %d`, here by the batch size in one statement).
            tx.update_where(
                "djmdSongPlaylist",
                "TrackNo = TrackNo + ?",
                &[Value::Integer(content_ids.len() as i64)],
                "PlaylistID = ? AND TrackNo >= ?",
                &[Value::Text(p.id.clone()), Value::Integer(pos)],
            )?;
            pos
        }
        _ => n0 + 1,
    };
    let mut entry_ids = Vec::new();
    let rows: Vec<_> = content_ids
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let id = new_uuid();
            entry_ids.push(id.clone());
            entry_columns(&id, &p.id, c.as_str(), start + i as i64, &new_uuid())
        })
        .collect();
    tx.insert_batch("djmdSongPlaylist", &rows)?;
    tx.commit()?;
    // The playlist's Timestamp becomes the time of the add (playlist-create.md §5, verified).
    xml_sync::sync(lib, std::slice::from_ref(&p.id))?;
    Ok((
        Effect::EntriesAdded { entry_ids: entry_ids.clone() },
        Some(Command::RemoveFromPlaylist { playlist: playlist.clone(), entry_ids }),
    ))
}

/// Shifts the entries after a removed `TrackNo` down by one (one builder update, one USN, spent
/// even when no row follows).
// hypothesis: rekordbox's range template `update __TABLE_NAME__ set __COLUMN__ = __COLUMN__ - 1,
// rb_data_status = …, rb_local_usn = __RB_LOCAL_USN__ … where __COLUMN__ > %d and __COLUMN__ <= %d
// __WHERE_CLAUSE__` (binary string, untraced) is what keeps TrackNo contiguous.
pub(crate) fn close_track_gap(tx: &WriteTxn<'_>, playlist: &str, removed_track_no: i64) -> Result<()> {
    tx.update_where(
        "djmdSongPlaylist",
        "TrackNo = TrackNo - 1",
        &[],
        "PlaylistID = ? AND TrackNo > ?",
        &[Value::Text(playlist.to_owned()), Value::Integer(removed_track_no)],
    )?;
    Ok(())
}

/// hypothesis (not traced): DELETE-ROWS over the entry IDs (one USN), then one gap-closing shift
/// per removed entry, highest `TrackNo` first.
pub(crate) fn remove_entries(lib: &mut Library, playlist: &DbId, entry_ids: &[String]) -> Result<(Effect, Option<Command>)> {
    let p = require_plain_playlist(lib.connection(), playlist.as_str())?;
    let all = entries(lib.connection(), &p.id)?;
    let mut removed: Vec<EntrySnapshot> = Vec::new();
    for id in entry_ids {
        let e = all.iter().find(|e| &e.id == id).ok_or_else(|| Error::NotFound { what: "playlist entry", id: id.clone() })?;
        if !removed.contains(e) {
            removed.push(e.clone());
        }
    }
    if removed.is_empty() {
        return Ok((Effect::Unchanged, None));
    }
    let keys: Vec<Value> = removed.iter().map(|e| Value::Text(e.id.clone())).collect();
    let tx = lib.writer.transaction()?;
    tx.delete_rows("djmdSongPlaylist", "ID", &keys)?;
    let mut by_track_desc = removed.clone();
    by_track_desc.sort_by_key(|e| std::cmp::Reverse(e.track_no));
    for e in &by_track_desc {
        close_track_gap(&tx, &p.id, e.track_no)?;
    }
    tx.commit()?;
    xml_sync::sync(lib, std::slice::from_ref(&p.id))?;
    removed.sort_by_key(|e| e.track_no);
    Ok((Effect::Done, Some(Command::RestoreEntries { playlist: playlist.clone(), entries: removed })))
}

/// Inverse of [`remove_entries`]: per entry, lowest old `TrackNo` first, a shift of the tail
/// (`TrackNo + 1`, one USN) and the insert with the original row ID and UUID (one USN).
pub(crate) fn restore_entries(lib: &mut Library, playlist: &DbId, list: &[EntrySnapshot]) -> Result<(Effect, Option<Command>)> {
    let p = require_plain_playlist(lib.connection(), playlist.as_str())?;
    let mut sorted = list.to_vec();
    sorted.sort_by_key(|e| e.track_no);
    let tx = lib.writer.transaction()?;
    let mut restored = Vec::new();
    for e in &sorted {
        if !content_is_live(tx.connection(), e.content_id.as_str()) {
            continue;
        }
        let n = live_entry_count(tx.connection(), &p.id)?;
        let pos = e.track_no.clamp(1, n + 1);
        tx.update_where(
            "djmdSongPlaylist",
            "TrackNo = TrackNo + 1",
            &[],
            "PlaylistID = ? AND TrackNo >= ?",
            &[Value::Text(p.id.clone()), Value::Integer(pos)],
        )?;
        let id = if any_row_with_id(&tx, "djmdSongPlaylist", &e.id)? { new_uuid() } else { e.id.clone() };
        tx.insert("djmdSongPlaylist", &entry_columns(&id, &p.id, e.content_id.as_str(), pos, &e.uuid))?;
        restored.push(id);
    }
    tx.commit()?;
    if restored.is_empty() {
        return Ok((Effect::Unchanged, None));
    }
    xml_sync::sync(lib, std::slice::from_ref(&p.id))?;
    Ok((
        Effect::EntriesAdded { entry_ids: restored.clone() },
        Some(Command::RemoveFromPlaylist { playlist: playlist.clone(), entry_ids: restored }),
    ))
}

/// One entry from `TrackNo` `from` to `to`: shift the entries in between (one USN), then set the
/// moved entry (one USN).
// hypothesis: rekordbox's binary templates `… set __COLUMN__ = __COLUMN__ + 1 … where
// __COLUMN__ >= %d and __COLUMN__ < %d …` / `… - 1 … where __COLUMN__ > %d and __COLUMN__ <= %d …`
// and `… set __COLUMN__ = %d … where ID = %lu …` (untraced), one bump per statement.
fn move_entry(tx: &WriteTxn<'_>, playlist: &str, entry_id: &str, from: i64, to: i64) -> Result<()> {
    let p = Value::Text(playlist.to_owned());
    if from > to {
        tx.update_where("djmdSongPlaylist", "TrackNo = TrackNo + 1", &[], "PlaylistID = ? AND TrackNo >= ? AND TrackNo < ?", &[
            p,
            Value::Integer(to),
            Value::Integer(from),
        ])?;
    } else {
        tx.update_where("djmdSongPlaylist", "TrackNo = TrackNo - 1", &[], "PlaylistID = ? AND TrackNo > ? AND TrackNo <= ?", &[
            p,
            Value::Integer(from),
            Value::Integer(to),
        ])?;
    }
    tx.update_where("djmdSongPlaylist", "TrackNo = ?", &[Value::Integer(to)], "ID = ?", &[Value::Text(entry_id.to_owned())])?;
    Ok(())
}

/// Reorders a playlist to `order` (a permutation of its entry IDs) with single-entry moves:
/// for each position from the top, the entry that belongs there is moved up into it.
pub(crate) fn reorder(lib: &mut Library, playlist: &DbId, order: &[String]) -> Result<(Effect, Option<Command>)> {
    let p = require_plain_playlist(lib.connection(), playlist.as_str())?;
    let current = entries(lib.connection(), &p.id)?;
    let mut list: Vec<String> = current.iter().map(|e| e.id.clone()).collect();
    let wanted = order.to_vec();
    let mut check = wanted.clone();
    check.sort();
    let mut have = list.clone();
    have.sort();
    if check != have {
        return Err(Error::Invalid(format!("order is not a permutation of the entries of {}", p.id)));
    }
    let contiguous = current.iter().enumerate().all(|(i, e)| e.track_no == i as i64 + 1);
    if contiguous && list == wanted {
        return Ok((Effect::Unchanged, None));
    }
    let tx = lib.writer.transaction()?;
    if !contiguous {
        // hypothesis: TrackNo gaps (e.g. left by another writer) are closed first, one statement.
        let mut case = String::from("TrackNo = CASE ID");
        let mut params = Vec::new();
        for (i, e) in current.iter().enumerate() {
            case.push_str(" WHEN ? THEN ?");
            params.push(Value::Text(e.id.clone()));
            params.push(Value::Integer(i as i64 + 1));
        }
        case.push_str(" ELSE TrackNo END");
        tx.update_where("djmdSongPlaylist", &case, &params, "PlaylistID = ?", &[Value::Text(p.id.clone())])?;
    }
    for i in 0..wanted.len() {
        if list[i] != wanted[i] {
            let from = list.iter().position(|x| x == &wanted[i]).expect("permutation");
            move_entry(&tx, &p.id, &wanted[i], from as i64 + 1, i as i64 + 1)?;
            let moved = list.remove(from);
            list.insert(i, moved);
        }
    }
    tx.commit()?;
    // hypothesis: a new track order counts as a change of the list (Timestamp).
    xml_sync::sync(lib, std::slice::from_ref(&p.id))?;
    Ok((Effect::Done, Some(Command::ReorderPlaylist { playlist: playlist.clone(), order: current.into_iter().map(|e| e.id).collect() })))
}

/// The entry order after moving `entry_ids` (in their current relative order) to before the
/// entry now at `position` (1-based; past the end appends). For [`Command::ReorderPlaylist`].
pub fn order_after_move(conn: &Connection, playlist: &str, entry_ids: &[String], position: u32) -> Result<Vec<String>> {
    let current: Vec<String> = entries(conn, playlist)?.into_iter().map(|e| e.id).collect();
    let pos = (position.max(1) as usize - 1).min(current.len());
    let before: Vec<&String> = current[..pos].iter().filter(|id| !entry_ids.contains(id)).collect();
    let moving: Vec<&String> = current.iter().filter(|id| entry_ids.contains(id)).collect();
    let after: Vec<&String> = current[pos..].iter().filter(|id| !entry_ids.contains(id)).collect();
    Ok(before.into_iter().chain(moving).chain(after).cloned().collect())
}
