//! Op: import player histories from a device. Spec: `docs/re/ops/import-device.md` § Histories
//! (binary-derived, no trace).
//!
//! `DatabaseMediator::importHistoryFromDeviceLocal` @0x100c535c4:
//! 1. Each device history's tracks are mapped to collection tracks with
//!    `getMainTrackByDeviceTrack` (device `masterDbId` = this DBID → local `ID` =
//!    `masterContentId`). Unmapped tracks are dropped; a history with none left is skipped.
//!    Histories do not import tracks.
//! 2. A history already listed in the stick's `ImportedHistory.xml` with the same content hash is
//!    skipped ([`ImportedHistory`]).
//! 3. Today's history folders are found or created (`getHistoryFolder` @0x1017f385c: year folder
//!    `"YYYY"`, then month folder `"M"`), and each history becomes `HISTORY YYYY-MM-DD`, with
//!    ` (n)` appended when that name exists in the month folder (`createNewHistory` →
//!    `CloudAgentAPI::HistoryIF::addNewHistory` @0x100a6388c).
//! 4. The tracks go into `djmdSongHistory` (`insertTrackToHistory` @0x100a63fc8) and each
//!    track's `DJPlayCount` grows by its number of plays (`updateDjPlayCount` @0x1017f3b94).
//!
//! The stick side (writing `ImportedHistory.xml`, optionally deleting the device histories) is
//! the caller's job; [`HistoryImportReport::imported`] lists what to record.

use crate::{Command, DbId, Effect, Library, Result};
use mb_masterdb::WriteTxn;
use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::write::{new_rid32, new_uuid};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// One played track of a device history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceHistoryEntry {
    pub device_content_id: u32,
    pub master_db_id: u32,
    pub master_content_id: u32,
    pub title: String,
}

/// A history list on a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceHistory {
    /// The history's ID in its device database.
    pub id: u32,
    /// From exportLibrary.db (`DEVICE-LIBRARY-PLUS`) rather than export.pdb (`DEVICE-LIBRARY`).
    pub plus: bool,
    pub name: String,
    pub entries: Vec<DeviceHistoryEntry>,
}

/// Histories from an export.pdb, in ID order.
pub fn device_histories(db: &mb_pdb::ExportDb) -> Vec<DeviceHistory> {
    let tracks: BTreeMap<u32, &mb_pdb::tables::Track> = db.tracks.iter().map(|t| (t.id, t)).collect();
    let mut out: Vec<DeviceHistory> = db
        .histories
        .iter()
        .map(|h| DeviceHistory { id: h.0.id, plus: false, name: h.0.name.clone(), entries: Vec::new() })
        .collect();
    out.sort_by_key(|h| h.id);
    let mut entries: Vec<&mb_pdb::tables::HistoryEntry> = db.history_entries.iter().collect();
    entries.sort_by_key(|e| (e.history_id, e.seq));
    for e in entries {
        let Some(h) = out.iter_mut().find(|h| h.id == e.history_id) else { continue };
        let t = tracks.get(&e.track_id);
        h.entries.push(DeviceHistoryEntry {
            device_content_id: e.track_id,
            master_db_id: t.map_or(0, |t| t.master_db_id),
            master_content_id: t.map_or(0, |t| t.master_content_id),
            title: t.map(|t| t.title.clone()).unwrap_or_default(),
        });
    }
    out
}

/// `PIONEER/rekordbox/ImportedHistory.xml`: which device histories were imported, per database,
/// with a hash of their content (`DeviceHistory::ImportedHistoryManager`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportedHistory {
    /// (history ID, content hash) for export.pdb histories.
    pub classic: Vec<(u32, String)>,
    /// Same for exportLibrary.db histories.
    pub plus: Vec<(u32, String)>,
}

impl ImportedHistory {
    /// Reads the file rekordbox writes (`deserealizeFromXml` @0x101a8c558). Unknown content is
    /// ignored.
    pub fn parse(text: &str) -> Self {
        let mut out = Self::default();
        let mut plus = false;
        let mut current: Option<u32> = None;
        for line in text.lines().map(str::trim) {
            if line.starts_with("<DEVICE-LIBRARY-PLUS") {
                plus = true;
            } else if line.starts_with("<DEVICE-LIBRARY") {
                plus = false;
            }
            if let Some(rest) = line.strip_prefix("<HISTORY id=\"") {
                current = rest.split('"').next().and_then(|v| v.parse().ok());
            }
            if let (Some(id), Some(rest)) = (current, line.strip_prefix("<CONTENTS>")) {
                let hash = rest.trim_end_matches("</CONTENTS>").to_owned();
                if plus { out.plus.push((id, hash)) } else { out.classic.push((id, hash)) }
                current = None;
            }
        }
        out
    }

    /// The bytes rekordbox writes: `juce::XmlElement::writeTo` with the default format (UTF-8
    /// declaration, an empty line, two-space indentation, CRLF line ends;
    /// `ImportedHistoryManager::serializeToXml` @0x101a8c8d0, `History::serealizeToXml`
    /// @0x101a8bba0). *Hypothesis* for the exact layout: no fixture has this file.
    pub fn to_xml(&self) -> String {
        let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n<IMPORTED-HISTORY>\r\n");
        for (tag, list) in [("DEVICE-LIBRARY", &self.classic), ("DEVICE-LIBRARY-PLUS", &self.plus)] {
            if list.is_empty() {
                s += &format!("  <{tag}/>\r\n");
                continue;
            }
            s += &format!("  <{tag}>\r\n");
            for (id, hash) in list {
                s += &format!("    <HISTORY id=\"{id}\">\r\n      <CONTENTS>{hash}</CONTENTS>\r\n    </HISTORY>\r\n");
            }
            s += &format!("  </{tag}>\r\n");
        }
        s += "</IMPORTED-HISTORY>\r\n";
        s
    }

    /// `ImportedHistory::exist` @0x101a8c094: same database, history ID and hash.
    pub fn contains(&self, plus: bool, id: u32, hash: &str) -> bool {
        let list = if plus { &self.plus } else { &self.classic };
        list.iter().any(|(i, h)| *i == id && h == hash)
    }

    pub fn add(&mut self, plus: bool, id: u32, hash: String) {
        if !self.contains(plus, id, &hash) {
            if plus { self.plus.push((id, hash)) } else { self.classic.push((id, hash)) }
        }
    }
}

/// `ImportedHistoryManager::getHash` @0x101a8cd08: SHA-256 (lower-case hex) of the
/// concatenated `%08x` master DB ID and master content ID of every mapped track, in play order.
/// *Hypothesis:* which ID arrays the caller passes (master content ID vs. device content ID);
/// for mapped tracks the master content ID is the collection ID.
pub fn history_hash(entries: &[(u32, u32)]) -> String {
    let mut text = String::new();
    for (dbid, cid) in entries {
        text += &format!("{dbid:08x}{cid:08x}");
    }
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HistoryImportReport {
    /// (device history, its database is Plus, local history ID, hash) for every imported list:
    /// what the caller records in `ImportedHistory.xml`.
    pub imported: Vec<(u32, bool, DbId, String)>,
    /// Device histories skipped: already imported, or no track of them is in the collection.
    pub skipped: Vec<u32>,
    /// Play counts added, per content ID (for undo).
    pub play_counts: Vec<(DbId, i64)>,
}

pub(crate) fn import(
    lib: &mut Library,
    histories: &[DeviceHistory],
    already: &ImportedHistory,
    now: &LocalTime,
) -> Result<(Effect, Option<Command>)> {
    let dbid: u32 = lib
        .connection()
        .query_row("SELECT DBID FROM djmdProperty", [], |r| r.get::<_, Option<String>>(0))?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut report = HistoryImportReport::default();
    let mut plays: BTreeMap<String, i64> = BTreeMap::new();
    let mut todo: Vec<(&DeviceHistory, Vec<String>, String)> = Vec::new();
    for h in histories {
        let mut local = Vec::new();
        let mut pairs = Vec::new();
        for e in &h.entries {
            if e.master_db_id == dbid && e.master_content_id != 0 && live_track(lib, e.master_content_id)? {
                local.push(e.master_content_id.to_string());
                pairs.push((e.master_db_id, e.master_content_id));
            }
        }
        let hash = history_hash(&pairs);
        if local.is_empty() || already.contains(h.plus, h.id, &hash) {
            report.skipped.push(h.id);
            continue;
        }
        todo.push((h, local, hash));
    }
    if todo.is_empty() {
        return Ok((Effect::HistoriesImported(report), None));
    }

    let tx = lib.writer.transaction()?;
    let month = history_folder(&tx, now)?;
    let base = format!("HISTORY {:04}-{:02}-{:02}", now.year, now.month, now.day);
    let first = next_suffix(&tx, &month, &base)?;
    let mut created = Vec::new();
    for (suffix, (h, local, hash)) in (first..).zip(&todo) {
        let name = if suffix > 0 { format!("{base} ({suffix})") } else { base.clone() };
        let id = add_history(&tx, &name, 0, &month, now)?;
        for (n, cid) in local.iter().enumerate() {
            // `insertTrackToHistory`: one ORM save per track (`SaveNewSongHistory`), `TrackNo` =
            // max + 1; `ID` and `UUID` are two v4 UUIDs like `djmdSongPlaylist` rows.
            tx.insert(
                "djmdSongHistory",
                &[
                    ("ID", Value::Text(new_uuid())),
                    ("HistoryID", Value::Text(id.clone())),
                    ("ContentID", Value::Text(cid.clone())),
                    ("TrackNo", Value::Integer(n as i64 + 1)),
                    ("UUID", Value::Text(new_uuid())),
                ],
            )?;
            *plays.entry(cid.clone()).or_default() += 1;
        }
        report.imported.push((h.id, h.plus, DbId(id.clone()), hash.clone()));
        created.push(DbId(id));
    }
    for (cid, n) in &plays {
        let old: i64 = tx
            .connection()
            .query_row("SELECT DJPlayCount FROM djmdContent WHERE rb_local_deleted = 0 AND ID = ?1", [cid], |r| {
                r.get::<_, Option<i64>>(0)
            })?
            .unwrap_or(0);
        tx.update("djmdContent", "ID", Value::Text(cid.clone()), &[("DJPlayCount", Value::Integer(old + n))])?;
        report.play_counts.push((DbId(cid.clone()), *n));
    }
    tx.commit()?;
    let inverse = Command::UndoHistoryImport { histories: created, play_counts: report.play_counts.clone() };
    Ok((Effect::HistoriesImported(report), Some(inverse)))
}

/// Inverse: the imported histories and their entries go, play counts drop back. Empty date
/// folders stay (rekordbox never removes them either).
pub(crate) fn undo(lib: &mut Library, histories: &[DbId], play_counts: &[(DbId, i64)]) -> Result<(Effect, Option<Command>)> {
    let tx = lib.writer.transaction()?;
    let ids: Vec<Value> = histories.iter().map(|h| Value::Text(h.0.clone())).collect();
    tx.delete_rows("djmdSongHistory", "HistoryID", &ids)?;
    tx.delete_rows("djmdHistory", "ID", &ids)?;
    for (cid, n) in play_counts {
        let old: i64 = tx
            .connection()
            .query_row("SELECT DJPlayCount FROM djmdContent WHERE rb_local_deleted = 0 AND ID = ?1", [&cid.0], |r| {
                r.get::<_, Option<i64>>(0)
            })?
            .unwrap_or(0);
        tx.update("djmdContent", "ID", Value::Text(cid.0.clone()), &[("DJPlayCount", Value::Integer((old - n).max(0)))])?;
    }
    tx.commit()?;
    Ok((Effect::Done, None))
}

/// Local wall-clock time of the import (rekordbox uses `juce::Time::getCurrentTime`, local).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl LocalTime {
    /// `%04d-%02d-%02d %02d:%02d:%02d`, the `DateCreated` format of `addNewHistory`.
    fn date_created(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

fn live_track(lib: &Library, id: u32) -> Result<bool> {
    Ok(lib.connection().query_row(
        "SELECT count(*) FROM djmdContent WHERE rb_local_deleted = 0 AND ID = ?1",
        [id.to_string()],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// `getHistoryFolder`: the year folder (`ID` = year) and its month folder (`ID` = year × 100 +
/// month), created when missing. Returns the month folder ID.
fn history_folder(tx: &WriteTxn<'_>, now: &LocalTime) -> Result<String> {
    let year = now.year.to_string();
    if !folder_exists(tx, &year)? {
        add_history(tx, &year, 1, "root", now)?;
    }
    let month_id = (now.year as i64 * 100 + i64::from(now.month)).to_string();
    if !folder_exists(tx, &month_id)? {
        add_history(tx, &now.month.to_string(), 1, &year, now)?;
    }
    Ok(month_id)
}

fn folder_exists(tx: &WriteTxn<'_>, id: &str) -> Result<bool> {
    Ok(tx.connection().query_row(
        "SELECT count(*) FROM djmdHistory WHERE rb_local_deleted = 0 AND ID = ?1 AND Attribute = 1",
        [id],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// The next ` (n)` for `base` among the month folder's children (0 = the plain name is free).
fn next_suffix(tx: &WriteTxn<'_>, parent: &str, base: &str) -> Result<u32> {
    let mut stmt = tx
        .connection()
        .prepare("SELECT Name FROM djmdHistory WHERE rb_local_deleted = 0 AND ParentID = ?1")?;
    let names: Vec<String> = stmt
        .query_map([parent], |r| r.get::<_, Option<String>>(0))?
        .filter_map(|n| n.ok().flatten())
        .collect();
    let mut next = 0;
    for n in names.iter().filter(|n| n.starts_with(base)) {
        let rest = &n[base.len()..];
        match rest.strip_prefix(" (").and_then(|r| r.strip_suffix(')')).and_then(|v| v.parse::<u32>().ok()) {
            Some(v) => next = next.max(v + 1),
            None if rest.is_empty() => next = next.max(1),
            None => {}
        }
    }
    Ok(next)
}

/// `addNewHistory`: `Seq` = max sibling `Seq` + 1; IDs: year folder = the year, month folder =
/// year × 100 + month (both with the ID as `UUID`), a history = a random u32 ≥ 3000 with a v4
/// UUID; `DateCreated` = local `YYYY-MM-DD HH:MM:SS`.
fn add_history(tx: &WriteTxn<'_>, name: &str, attribute: i64, parent: &str, now: &LocalTime) -> Result<String> {
    let (id, uuid) = if attribute == 1 && parent == "root" {
        (name.to_owned(), name.to_owned())
    } else if attribute == 1 {
        let id = (parent.parse::<i64>().unwrap_or(0) * 100 + name.parse::<i64>().unwrap_or(0)).to_string();
        (id.clone(), id)
    } else {
        let id = loop {
            let v = new_rid32()?;
            if v >= 3000 && !tx.id_exists("djmdHistory", &v.to_string())? {
                break v.to_string();
            }
        };
        (id, new_uuid())
    };
    let seq: i64 = tx
        .connection()
        .query_row(
            "SELECT max(Seq) FROM djmdHistory WHERE rb_local_deleted = 0 AND ParentID = ?1",
            [parent],
            |r| r.get::<_, Option<i64>>(0),
        )?
        .unwrap_or(0);
    tx.insert(
        "djmdHistory",
        &[
            ("ID", Value::Text(id.clone())),
            ("Seq", Value::Integer(seq + 1)),
            ("Name", Value::Text(name.to_owned())),
            ("Attribute", Value::Integer(attribute)),
            ("ParentID", Value::Text(parent.to_owned())),
            ("DateCreated", Value::Text(now.date_created())),
            ("UUID", Value::Text(uuid)),
        ],
    )?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_round_trip_and_hash() {
        let mut ih = ImportedHistory::default();
        ih.add(false, 3, history_hash(&[(417703505, 12), (417703505, 99)]));
        let xml = ih.to_xml();
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n<IMPORTED-HISTORY>\r\n"));
        assert!(xml.contains("  <DEVICE-LIBRARY-PLUS/>\r\n"));
        assert_eq!(ImportedHistory::parse(&xml), ih);
        assert_eq!(ih.classic[0].1.len(), 64);
        assert!(ih.contains(false, 3, &ih.classic[0].1.clone()));
        assert!(!ih.contains(true, 3, &ih.classic[0].1.clone()));
    }
}
