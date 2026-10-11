//! Replay of the oracle export history (`fixtures/oracle-a/rbtest`): rekordbox 7.2.19 exported
//! playlist `oracle-small` (5 tracks) to an empty stick, then, in a later session, `oracle-all`
//! (157 tracks, the first 5 included). The functions here rebuild the operation sequence from the
//! rows of rekordbox's file and run it through [`crate::writer::Exporter`], so the output can be
//! compared byte for byte with rekordbox's. Spec: `docs/re/pdb.md` § Writer verification.

use std::collections::BTreeMap;

use crate::Result;
use crate::engine::Db;
use crate::library::{ExportDb, ExtDb};
use crate::page::Pdb;
use crate::schema::DbKind;
use crate::tables::TypedRow;
use crate::tables::*;
use crate::writer::{Exporter, Lookups, insert_row, my_tag_tree_order};

/// The session plan of the oracle stick, fitted to its transaction ids (`docs/re/pdb.md`
/// § Writer verification). Every reopen is a close and an open without writes; each advances the
/// transaction id by two.
#[derive(Debug, Clone)]
pub struct OraclePlan {
    /// Reopens between the first export and the second one (the first is the session break).
    pub reopen_between: usize,
    /// Write transactions between the second export's playlist inserts and its reorder whose
    /// pages are rewritten later (contents unknown, only their transaction ids are visible).
    pub hidden_writes: usize,
    /// Reopens after the second export.
    pub reopen_after: usize,
}

impl Default for OraclePlan {
    fn default() -> Self {
        Self {
            reopen_between: 11,
            hidden_writes: 1,
            reopen_after: 1,
        }
    }
}

fn entries_by_playlist(rows: &ExportDb) -> BTreeMap<u32, Vec<PlaylistEntry>> {
    let mut m: BTreeMap<u32, Vec<PlaylistEntry>> = BTreeMap::new();
    for e in &rows.playlist_entries {
        m.entry(e.playlist_id).or_default().push(*e);
    }
    for v in m.values_mut() {
        v.sort_by_key(|e| e.entry_index);
    }
    m
}

/// export.pdb of the oracle stick, replayed with the default plan.
pub fn oracle_export(oracle: &Pdb) -> Result<Vec<u8>> {
    oracle_export_with(oracle, &OraclePlan::default())
}

/// export.pdb of the oracle stick, replayed with an explicit plan.
pub fn oracle_export_with(oracle: &Pdb, plan: &OraclePlan) -> Result<Vec<u8>> {
    let rows = ExportDb::read(oracle)?;
    let lookups = Lookups::from_export(&rows);
    let tracks: BTreeMap<u32, Track> = rows.tracks.iter().map(|t| (t.id, t.clone())).collect();
    let entries = entries_by_playlist(&rows);
    let mut playlists: Vec<PlaylistNode> = rows.playlists.clone();
    playlists.sort_by_key(|p| p.0.id);

    let mut defaults = ExportDb {
        colors: rows.colors.clone(),
        menu_items: rows.menu_items.clone(),
        categories: rows.categories.clone(),
        sorts: rows.sorts.clone(),
        ..Default::default()
    };
    let mut prop = rows.properties[0].clone();
    prop.track_count = 0;
    defaults.properties = vec![prop];

    let mut ex = Exporter::create(&defaults)?;
    let (small, all) = (&playlists[0], &playlists[1]);
    let small_entries = entries.get(&small.0.id).cloned().unwrap_or_default();
    let all_entries = entries.get(&all.0.id).cloned().unwrap_or_default();

    // Session 1: the first list (inserted and rewritten in one transaction), then its tracks.
    let mut first = small.clone();
    first.0.seq = 0;
    ex.create_first_playlist(&first)?;
    for e in &small_entries {
        ex.add_track(&tracks[&e.track_id], &lookups)?;
        ex.add_entry(e)?;
    }

    // Session 2: the existing list is emptied, the new list is inserted after it, the old
    // list's tracks (all unchanged) are re-added, then the new list moves to position 0 and its
    // own tracks are exported. Two write transactions leave only their ids (`hidden_writes`).
    for _ in 0..plan.reopen_between {
        ex.reopen();
    }
    ex.clear_playlist(small.0.id)?;
    ex.touch_playlist(small.0.id)?;
    let mut second = all.clone();
    second.0.seq = 1;
    ex.insert_playlist(&second)?;
    for e in &small_entries {
        ex.add_entry(e)?;
    }
    for _ in 0..plan.hidden_writes {
        ex.touch_playlist(small.0.id)?;
    }
    let mut moved = first.clone();
    moved.0.seq = 1;
    let mut top = all.clone();
    top.0.seq = 0;
    ex.update_playlists(&[moved, top])?;
    for e in &all_entries {
        if !ex.has_track(e.track_id) {
            ex.add_track(&tracks[&e.track_id], &lookups)?;
        }
        ex.add_entry(e)?;
    }
    for _ in 0..plan.reopen_after {
        ex.reopen();
    }
    Ok(ex.finish())
}

/// exportExt.pdb of the oracle stick: the file is created with its DBPROPERTYOPTION row, then
/// the My Tag tree is written (delete all, insert all) once per sync.
pub fn oracle_ext(oracle: &Pdb) -> Result<Vec<u8>> {
    let rows = ExtDb::read(oracle)?;
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
    let tags = my_tag_tree_order(&rows.tags);
    let mut refs = Vec::new();
    // Session 1: one sync. Session 2 (after the same reopens as export.pdb): two syncs.
    for sync in 0..3 {
        if sync == 1 {
            for _ in 0..OraclePlan::default().reopen_between {
                db.reopen();
            }
        }
        let old = std::mem::take(&mut refs);
        if !old.is_empty() {
            db.txn(|db| {
                for r in &old {
                    db.delete(MyTag::TABLE_TYPE, *r)?;
                }
                Ok(())
            })?;
        }
        refs = db.txn(|db| {
            tags.iter()
                .map(|t| insert_row(db, t))
                .collect::<Result<Vec<_>>>()
        })?;
    }
    db.reopen();
    db.close();
    Ok(db.to_bytes())
}
