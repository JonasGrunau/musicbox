//! Op: remove tracks from the collection. Spec: `docs/re/ops/remove-tracks.md` (verified for
//! never-synced rows: 14 tracks, trace L496–590).
//!
//! One `begin exclusive` transaction with ten DELETE-ROWS (one USN each, spent even when nothing
//! matches), the plain `djmdCue` delete by `ContentUUID` (no USN) and the `VideoAssociate`
//! clearing (one USN). Then, in autocommit mode, ORPHAN-CLEANUP over artist, album, genre, label,
//! key (one USN per table with orphans). Total: +11, +1 per orphan table.

use crate::{Command, DbId, Effect, Library, Result, playlist, xml_sync};
use mb_masterdb::rusqlite::types::Value;

/// The ten DELETE-ROWS of the transaction, in trace order: (table, key column).
pub const DELETE_ROWS: [(&str, &str); 10] = [
    ("djmdContent", "ID"),
    ("contentFile", "ContentID"),
    ("djmdRecommendLike", "ContentID1"),
    ("djmdRecommendLike", "ContentID2"),
    ("djmdSongPlaylist", "ContentID"),
    ("djmdSongHistory", "ContentID"),
    ("djmdSongMyTag", "ContentID"),
    ("djmdSongSampler", "ContentID"),
    ("djmdMixerParam", "ContentID"),
    ("djmdCloudExportSongPlaylist", "ContentID"),
];

pub(crate) fn remove_tracks(lib: &mut Library, ids: &[DbId]) -> Result<(Effect, Option<Command>)> {
    if ids.is_empty() {
        return Ok((Effect::Unchanged, None));
    }
    let keys: Vec<Value> = ids.iter().map(|i| Value::Text(i.0.clone())).collect();
    let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    // Pre-reads (UI thread): the cue rows go by content UUID; the playlist memberships decide
    // which playlists change (masterPlaylists6.xml Timestamp, TrackNo gaps).
    let (uuids, memberships) = {
        let conn = lib.connection();
        let mut stmt = conn.prepare(&format!(
            "SELECT UUID FROM djmdContent WHERE rb_local_deleted = 0 AND ID IN ({placeholders}) AND UUID NOTNULL"
        ))?;
        let uuids: Vec<Value> = stmt
            .query_map(mb_masterdb::rusqlite::params_from_iter(keys.iter()), |r| r.get::<_, String>(0))?
            .map(|r| r.map(Value::Text))
            .collect::<mb_masterdb::rusqlite::Result<_>>()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT PlaylistID, TrackNo FROM djmdSongPlaylist WHERE rb_local_deleted = 0 AND ContentID IN ({placeholders}) ORDER BY PlaylistID, TrackNo DESC"
        ))?;
        let memberships: Vec<(String, i64)> = stmt
            .query_map(mb_masterdb::rusqlite::params_from_iter(keys.iter()), |r| {
                Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0)))
            })?
            .collect::<mb_masterdb::rusqlite::Result<_>>()?;
        (uuids, memberships)
    };

    let tx = lib.writer.transaction()?;
    for (i, (table, key_col)) in DELETE_ROWS.iter().enumerate() {
        tx.delete_rows(table, key_col, &keys)?;
        if i == 1 {
            // `DELETE FROM djmdCue WHERE ContentUUID IN(…)`: hard delete, no USN, no soft branch.
            // (`contentCue` is only read here.)
            tx.delete_unversioned("djmdCue", "ContentUUID", &uuids)?;
        }
    }
    // Bump spent even though nothing matches in the traced case (common.md rule 3).
    tx.update_where("djmdContent", "VideoAssociate = ''", &[], &format!("VideoAssociate IN ({})", vec!["?"; ids.len()].join(", ")), &keys)?;
    tx.commit()?;

    // After the commit, autocommit on the worker thread: ORPHAN-CLEANUP.
    let tx = lib.writer.transaction()?;
    tx.cleanup_orphans()?;
    tx.commit()?;

    // hypothesis (untraced: no removed track was in a playlist): the remaining entries of each
    // affected playlist are renumbered the way removing entries from a playlist does
    // (`playlist::close_track_gaps`), so that `TrackNo` stays 1…n.
    let mut touched: Vec<String> = Vec::new();
    if !memberships.is_empty() {
        let tx = lib.writer.transaction()?;
        for (playlist_id, track_no) in &memberships {
            playlist::close_track_gap(&tx, playlist_id, *track_no)?;
            if !touched.contains(playlist_id) {
                touched.push(playlist_id.clone());
            }
        }
        tx.commit()?;
        // hypothesis: a playlist whose member tracks change gets a new Timestamp (verified for
        // analysis completion, playlist-create.md §5).
        xml_sync::sync(lib, &touched)?;
    }
    Ok((Effect::Removed { ids: ids.to_vec() }, None))
}
