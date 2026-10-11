//! Keeps `masterPlaylists6.xml` in step with `djmdPlaylist` after every playlist change.
//! Spec: `docs/re/ops/playlist-create.md` §5.
//!
//! - One `NODE` per live playlist row. Root children in `Seq` order (verified).
//! - `Timestamp` = the last change of the list or of one of its tracks. Nodes this op changed
//!   (`touched`) get the current time; new nodes too; every other node keeps its value. A pure
//!   `Seq` shift does not count as a change: `30D40` kept its timestamp in oracle-a although
//!   both playlist creations rewrote its `Seq` (verified).
//! - `Lib_Type`, `CheckType` and unknown attributes of existing nodes are kept; new nodes get 0.
//!
//! rekordbox flushes the file lazily (its mtime is later than every timestamp in it); musicbox
//! writes it at the end of each op, and only when its content changes.

use crate::{Library, Result, now_ms};
use mb_masterdb::MasterPlaylists;
use mb_masterdb::playlists_xml::PlaylistXmlNode;
use std::collections::HashMap;

struct Row {
    id: String,
    parent: String,
    seq: i64,
    attribute: i64,
}

/// Rewrites the file from the live `djmdPlaylist` rows. `touched` are playlist IDs whose
/// `Timestamp` becomes now.
pub(crate) fn sync(lib: &Library, touched: &[String]) -> Result<()> {
    let rows: Vec<Row> = {
        let mut stmt = lib
            .connection()
            .prepare("SELECT ID, ParentID, Seq, Attribute FROM djmdPlaylist WHERE rb_local_deleted = 0 ORDER BY Seq, rowid")?;
        stmt.query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                parent: r.get::<_, Option<String>>(1)?.unwrap_or_else(|| "root".into()),
                seq: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                attribute: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            })
        })?
        .collect::<mb_masterdb::rusqlite::Result<_>>()?
    };
    let existing = MasterPlaylists::read(&lib.xml_path)?;
    let mut doc = existing.clone().unwrap_or_default();
    let mut old: HashMap<u64, PlaylistXmlNode> = doc.nodes.drain(..).map(|n| (n.id, n)).collect();
    let now = now_ms();

    // hypothesis: nested nodes are listed depth-first (pre-order), each folder followed by its
    // children in Seq order; only root children have been observed.
    let mut children: HashMap<&str, Vec<&Row>> = HashMap::new();
    for r in &rows {
        children.entry(r.parent.as_str()).or_default().push(r);
    }
    for list in children.values_mut() {
        list.sort_by_key(|r| r.seq);
    }
    let mut order: Vec<&Row> = Vec::new();
    let mut stack: Vec<&Row> = children.get("root").map(|v| v.iter().rev().copied().collect()).unwrap_or_default();
    while let Some(r) = stack.pop() {
        order.push(r);
        if let Some(kids) = children.get(r.id.as_str()) {
            stack.extend(kids.iter().rev().copied());
        }
    }
    for r in order {
        let Ok(id) = r.id.parse::<u64>() else { continue };
        let parent_id = if r.parent == "root" { 0 } else { r.parent.parse::<u64>().unwrap_or(0) };
        let mut node = old.remove(&id).unwrap_or_else(|| PlaylistXmlNode::new(id, parent_id, r.attribute, now));
        node.parent_id = parent_id;
        node.attribute = r.attribute;
        if touched.iter().any(|t| t == &r.id) {
            node.timestamp = now;
        }
        doc.nodes.push(node);
    }
    if existing.as_ref() != Some(&doc) {
        doc.write(&lib.xml_path)?;
    }
    Ok(())
}
