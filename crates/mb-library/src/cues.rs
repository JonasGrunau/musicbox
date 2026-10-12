//! Ops: add, delete and edit memory cues, hot cues and loops of a collection track.
//! Spec: `docs/re/ops/edit-cues.md` (binary-derived; no traced cue write exists yet).
//!
//! rekordbox keeps collection cues in the database only; the local ANLZ files are not touched
//! (`CAnalyzerIF::setDeviceCue` is reached for device tracks only). The USB export rebuilds the
//! device PCOB/PCO2 tags from these rows.
//!
//! Every cue change is three kinds of writes:
//! 1. the `djmdCue` row itself, written through the ORM without the library-sync columns: no USN,
//!    `rb_*` left at their defaults (`DaoBase<Cue, NoLibrarySyncSql>`);
//! 2. "library sync" of the track (`CueRepository::librarySyncByContentId` @0x1005c9650): every cue
//!    row gets the track's `ContentUUID`, and the `contentCue` row (`ID` = content UUID) is
//!    inserted or updated with the JSON array of all the track's cue rows, under one USN bump;
//! 3. `djmdContent.CueUpdated += 1` (COUNTER macro), and the playlists holding the track get a new
//!    `masterPlaylists6.xml` timestamp (`update_playlist_timestamp`).

use crate::{Command, DbId, Effect, Error, Library, Result, xml_sync};
use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::rusqlite::{Connection, OptionalExtension, params_from_iter};
use mb_masterdb::write::{new_rid32, new_uuid, timestamp};
use mb_masterdb::{CounterColumn, WriteTxn};
use std::collections::HashMap;

/// Every `djmdCue` column except `ID`, `ContentID` and the row-change columns, in the entity's
/// field order (`rb::middleware::db::rekordbox::Cue::Cue` @0x100c2a4bc), which is also the key
/// order of the `contentCue` JSON.
const JSON_FIELDS: [(&str, JsonKind); 23] = [
    ("ID", JsonKind::Id),
    ("ContentID", JsonKind::Id),
    ("ContentUUID", JsonKind::Text),
    ("InMsec", JsonKind::Int),
    ("InFrame", JsonKind::Int),
    ("InMpegFrame", JsonKind::Int),
    ("InMpegAbs", JsonKind::Int),
    ("InPointSeekInfo", JsonKind::Text),
    ("OutMsec", JsonKind::Int),
    ("OutFrame", JsonKind::Int),
    ("OutMpegFrame", JsonKind::Int),
    ("OutMpegAbs", JsonKind::Int),
    ("OutPointSeekInfo", JsonKind::Text),
    ("Kind", JsonKind::Int),
    ("Color", JsonKind::Int),
    ("ColorTableIndex", JsonKind::Int),
    ("ActiveLoop", JsonKind::Int),
    ("Comment", JsonKind::Text),
    ("BeatLoopSize", JsonKind::Int),
    ("CueMicrosec", JsonKind::Int),
    ("UUID", JsonKind::Text),
    ("created_at", JsonKind::Text),
    ("updated_at", JsonKind::Text),
];

#[derive(Clone, Copy)]
enum JsonKind {
    /// `Field<Id, unsigned>`: the number as a quoted string.
    Id,
    /// `Field<Integer, int>`: a bare number.
    Int,
    /// Varchar, Uuid and DateTime: `juce::String::quoted`.
    Text,
}

/// Which pad or list a cue belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CueSlot {
    /// Memory cue or memory loop (`Kind` 0; 4 marks an active memory loop when read back).
    Memory,
    /// Hot cue pad 0 = A … 15 (`Kind` 1–3 for A–C, 5–17 for D onwards; 4 is skipped).
    Hot(u8),
}

impl CueSlot {
    /// `djmdCue.Kind` (`djplay::DjplayDB::setHotCueData` @0x1001ff7f8: pads 0–2 → +1, 3–15 → +2).
    pub fn kind(self) -> i64 {
        match self {
            CueSlot::Memory => 0,
            CueSlot::Hot(n) if n < 3 => i64::from(n) + 1,
            CueSlot::Hot(n) => i64::from(n) + 2,
        }
    }

    pub fn from_kind(kind: i64) -> Option<Self> {
        match kind {
            -1 | 0 | 4 => Some(CueSlot::Memory),
            1..=3 => Some(CueSlot::Hot((kind - 1) as u8)),
            5..=17 => Some(CueSlot::Hot((kind - 2) as u8)),
            _ => None,
        }
    }

    pub fn letter(self) -> Option<char> {
        match self {
            CueSlot::Hot(n) if n < 26 => Some((b'A' + n) as char),
            _ => None,
        }
    }
}

/// A cue to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCue {
    pub slot: CueSlot,
    pub in_ms: u32,
    /// Loop end; `None` for a cue point.
    pub out_ms: Option<u32>,
    /// `Color`: memory-cue colour (0–7 in rekordbox's list); `None` = no colour.
    pub color: Option<u8>,
    /// `ColorTableIndex`: hot-cue colour code 1–64; `None` = default.
    pub color_table_index: Option<u8>,
    pub comment: String,
}

/// One live `djmdCue` row, typed for display and editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub id: String,
    pub content_id: String,
    pub slot: CueSlot,
    pub in_ms: u32,
    pub out_ms: Option<u32>,
    pub color: Option<u8>,
    pub color_table_index: Option<u8>,
    pub comment: String,
    pub active_loop: bool,
}

/// A change to one existing cue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CueEdit {
    /// `modifyCueColor` @0x100a65b04 (`None` is stored as 255, rekordbox's unsigned −1).
    Color(Option<u8>),
    /// `modifyCueColorTblIdx` @0x100a65d30.
    ColorTableIndex(u8),
    /// `modifyCueComment` @0x100a65c0c.
    Comment(String),
    /// `modifyCueOptionActiveLoop` @0x100a65e38.
    ActiveLoop(bool),
}

/// `(int)(ms * 0.15)`: the 1/150 s frame columns, truncated like rekordbox's C cast.
pub fn ms_to_frame(ms: u32) -> i64 {
    (f64::from(ms) * 0.15) as i64
}

fn int(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(n) => Some(*n),
        Value::Real(f) => Some(*f as i64),
        Value::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) => Some(s.clone()),
        Value::Integer(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A raw row as (column, value) pairs in table order.
type RawRow = Vec<(String, Value)>;

fn raw_rows(conn: &Connection, sql: &str, params: &[Value]) -> Result<Vec<RawRow>> {
    let mut stmt = conn.prepare(sql)?;
    let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
    let rows = stmt
        .query_map(params_from_iter(params.iter()), |r| {
            (0..names.len()).map(|i| Ok((names[i].clone(), r.get::<_, Value>(i)?))).collect::<mb_masterdb::rusqlite::Result<RawRow>>()
        })?
        .collect::<mb_masterdb::rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn get<'a>(row: &'a RawRow, col: &str) -> &'a Value {
    row.iter().find(|(c, _)| c == col).map_or(&Value::Null, |(_, v)| v)
}

fn typed(row: &RawRow) -> Option<Cue> {
    let kind = int(get(row, "Kind"))?;
    let out = int(get(row, "OutMsec")).unwrap_or(-1);
    Some(Cue {
        id: text(get(row, "ID"))?,
        content_id: text(get(row, "ContentID")).unwrap_or_default(),
        slot: CueSlot::from_kind(kind)?,
        in_ms: int(get(row, "InMsec")).unwrap_or(0).max(0) as u32,
        out_ms: (out > 0).then_some(out as u32),
        color: int(get(row, "Color")).filter(|c| (0..=127).contains(c)).map(|c| c as u8),
        color_table_index: int(get(row, "ColorTableIndex")).filter(|c| (1..=64).contains(c)).map(|c| c as u8),
        comment: text(get(row, "Comment")).unwrap_or_default(),
        active_loop: int(get(row, "ActiveLoop")).is_some_and(|a| a & 1 != 0),
    })
}

/// The live cues of one track, in row order.
pub fn load_for_content(conn: &Connection, content_id: &str) -> Result<Vec<Cue>> {
    let rows = raw_rows(
        conn,
        "SELECT * FROM djmdCue WHERE rb_local_deleted = 0 AND ContentID = ?1 ORDER BY rowid",
        &[Value::Text(content_id.to_owned())],
    )?;
    Ok(rows.iter().filter_map(typed).collect())
}

/// Every live cue, grouped by `ContentID`.
pub fn load_all(conn: &Connection) -> Result<HashMap<DbId, Vec<Cue>>> {
    let rows = raw_rows(conn, "SELECT * FROM djmdCue WHERE rb_local_deleted = 0 ORDER BY rowid", &[])?;
    let mut out: HashMap<DbId, Vec<Cue>> = HashMap::new();
    for cue in rows.iter().filter_map(typed) {
        out.entry(DbId(cue.content_id.clone())).or_default().push(cue);
    }
    Ok(out)
}

/// `juce::String::quoted`: adds a `"` at either end unless one is already there; no escaping.
fn juce_quoted(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".into();
    }
    let mut t = String::with_capacity(s.len() + 2);
    if !s.starts_with('"') {
        t.push('"');
    }
    t.push_str(s);
    if !s.ends_with('"') {
        t.push('"');
    }
    t
}

/// One cue row as rekordbox serialises it for `contentCue.Cues` (`Cue::ToJson` @0x100c2b5a0 via
/// `EntityCommon::ToJson`): `{` + `"Field":value` for every non-NULL column, comma-separated + `}`.
fn cue_json(row: &RawRow) -> String {
    let mut parts = Vec::new();
    for (name, kind) in JSON_FIELDS {
        let v = get(row, name);
        let value = match (kind, v) {
            (_, Value::Null) => continue,
            (JsonKind::Id, v) => juce_quoted(&text(v).unwrap_or_default()),
            (JsonKind::Int, v) => match int(v) {
                Some(n) => (n as i32).to_string(),
                None => continue,
            },
            (JsonKind::Text, v) => juce_quoted(&text(v).unwrap_or_default()),
        };
        parts.push(format!("{}:{value}", juce_quoted(name)));
    }
    format!("{{{}}}", parts.join(","))
}

/// `CueRepository::toCuesJsonArray`: `[` + rows joined by `,` + `]`.
fn cues_json(rows: &[RawRow]) -> String {
    format!("[{}]", rows.iter().map(cue_json).collect::<Vec<_>>().join(","))
}

/// `librarySyncByContentId` @0x1005c9650: stamps the track's UUID on its cue rows, then writes
/// the `contentCue` row (one USN bump).
fn library_sync(tx: &WriteTxn<'_>, content_id: &str) -> Result<()> {
    let conn = tx.connection();
    let content_uuid: Option<String> = conn
        .query_row("SELECT UUID FROM djmdContent WHERE ID = ?1 AND rb_local_deleted = 0", [content_id], |r| r.get(0))
        .optional()?
        .flatten();
    let Some(content_uuid) = content_uuid else {
        // The track is gone: `deleteContentCue` (not reached by these ops).
        return Ok(());
    };
    // `CueDao::SelectByContentId`: `SELECT * FROM djmdCue WHERE ContentID = …`, no ORDER BY.
    let select = "SELECT * FROM djmdCue WHERE ContentID = ?1 ORDER BY rowid";
    let rows = raw_rows(conn, select, &[Value::Text(content_id.to_owned())])?;
    for row in &rows {
        if text(get(row, "ContentUUID")).as_deref() != Some(content_uuid.as_str()) {
            // `Cue::UpdateCueData` (ID + updated_at) plus ContentUUID; ORM update without USN.
            conn.execute(
                "UPDATE djmdCue SET ContentUUID = ?1, updated_at = ?2 WHERE ID = ?3",
                [content_uuid.clone(), timestamp(), text(get(row, "ID")).unwrap_or_default()],
            )?;
        }
    }
    let rows = raw_rows(conn, select, &[Value::Text(content_id.to_owned())])?;
    let json = cues_json(&rows);
    let count = rows.len() as i64;
    let existing: Option<(String, Option<i64>)> = conn
        .query_row("SELECT ID, usn FROM contentCue WHERE ContentID = ?1 ORDER BY rowid LIMIT 1", [content_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let usn = tx.bump_usn()?;
    let now = timestamp();
    match existing {
        None => {
            // `ContentCue::NewData` + `LibrarySync::NewStatus` @0x101a734bc: new UUID, statuses 0,
            // created_at = updated_at; `rb_local_data_status` and `usn` are not set.
            conn.execute(
                "INSERT INTO contentCue (ID, ContentID, Cues, rb_cue_count, UUID, rb_data_status, rb_local_deleted, \
                 rb_local_synced, rb_local_usn, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, 0, ?6, ?7, ?7)",
                params_from_iter([
                    Value::Text(content_uuid),
                    Value::Text(content_id.to_owned()),
                    Value::Text(json),
                    Value::Integer(count),
                    Value::Text(new_uuid()),
                    Value::Integer(usn),
                    Value::Text(now),
                ]),
            )?;
        }
        Some((id, synced_usn)) => {
            // `LibrarySync::ToUpdateStatus` @0x101a73874: rb_data_status 257 when `usn` is set.
            let status = if synced_usn.is_some() { 257 } else { 0 };
            conn.execute(
                "UPDATE contentCue SET Cues = ?1, rb_cue_count = ?2, rb_data_status = ?3, rb_local_deleted = 0, \
                 rb_local_usn = ?4, updated_at = ?5 WHERE ID = ?6",
                params_from_iter([
                    Value::Text(json),
                    Value::Integer(count),
                    Value::Integer(status),
                    Value::Integer(usn),
                    Value::Text(now),
                    Value::Text(id),
                ]),
            )?;
        }
    }
    Ok(())
}

/// ORM update of option columns of one cue (`Cue::UpdateCueData` + set fields, no USN), followed
/// by library sync (`CueRepository::UpdateCue` @0x1005c77dc).
fn update_cue(tx: &WriteTxn<'_>, cue_id: &str, content_id: &str, columns: &[(&str, Value)]) -> Result<()> {
    let mut sets: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    for (c, v) in columns {
        values.push(v.clone());
        sets.push(format!("{c} = ?{}", values.len()));
    }
    values.push(Value::Text(timestamp()));
    sets.push(format!("updated_at = ?{}", values.len()));
    values.push(Value::Text(cue_id.to_owned()));
    tx.connection().execute(&format!("UPDATE djmdCue SET {} WHERE ID = ?{}", sets.join(", "), values.len()), params_from_iter(values))?;
    library_sync(tx, content_id)
}

/// Ends every cue op: `updateCueUpdated` (COUNTER), commit.
fn finish(tx: WriteTxn<'_>, content_id: &str) -> Result<()> {
    tx.increment_counter(content_id, CounterColumn::CueUpdated)?;
    Ok(tx.commit()?)
}

/// After the commit: the playlists holding the track get a new `masterPlaylists6.xml` timestamp.
fn touch_playlists(lib: &Library, content_id: &str) -> Result<()> {
    let playlists: Vec<String> = {
        let mut stmt = lib
            .connection()
            .prepare("SELECT DISTINCT PlaylistID FROM djmdSongPlaylist WHERE rb_local_deleted = 0 AND ContentID = ?1")?;
        stmt.query_map([content_id], |r| r.get(0))?.collect::<mb_masterdb::rusqlite::Result<_>>()?
    };
    if !playlists.is_empty() {
        xml_sync::sync(lib, &playlists)?;
    }
    Ok(())
}

fn require_track(lib: &Library, id: &str) -> Result<()> {
    let exists = lib
        .connection()
        .query_row("SELECT 1 FROM djmdContent WHERE ID = ?1 AND rb_local_deleted = 0", [id], |_| Ok(()))
        .optional()?
        .is_some();
    if exists { Ok(()) } else { Err(Error::NotFound { what: "track", id: id.to_owned() }) }
}

fn cue_row(lib: &Library, cue_id: &str) -> Result<RawRow> {
    raw_rows(lib.connection(), "SELECT * FROM djmdCue WHERE ID = ?1 AND rb_local_deleted = 0", &[Value::Text(cue_id.to_owned())])?
        .into_iter()
        .next()
        .ok_or_else(|| Error::NotFound { what: "cue", id: cue_id.to_owned() })
}

/// `DatabaseMediator::setCue` → `rekordboxDBController::setCue` @0x1017deef4 (type 3, master):
/// `CueIF::setCue` inserts the row (`Color` = −1, option columns NULL), then `addCueOption` when
/// a colour, colour code or comment is set, then `updateCueUpdated`.
pub(crate) fn add_cue(lib: &mut Library, content: &DbId, cue: &NewCue) -> Result<(Effect, Option<Command>)> {
    let out = insert_cue(lib, content, cue)?;
    touch_playlists(lib, content.as_str())?;
    Ok(out)
}

/// `rekordboxDBController::setCue` alone (vtable +0x448): the row, library sync, options and
/// `CueUpdated`, without the mediator's playlist timestamp. The rekordbox.xml import calls it
/// directly (`importRBXmlData` @0x101a161f0).
pub(crate) fn insert_cue(lib: &mut Library, content: &DbId, cue: &NewCue) -> Result<(Effect, Option<Command>)> {
    require_track(lib, content.as_str())?;
    if let Some(out) = cue.out_ms
        && out <= cue.in_ms
    {
        return Err(Error::Invalid(format!("loop end {out} ms is not after its start {} ms", cue.in_ms)));
    }
    if let CueSlot::Hot(n) = cue.slot
        && n > 15
    {
        return Err(Error::Invalid(format!("hot cue pad {n} is not 0…15")));
    }
    let tx = lib.writer.transaction()?;
    // `CueIF::setCue` @0x100a79374: ID = random u32 other than 0 and 0xFFFFFFFF.
    let id = loop {
        let candidate = new_rid32()?.to_string();
        if !tx.id_exists("djmdCue", &candidate)? {
            break candidate;
        }
    };
    let (out_msec, out_frame) = match cue.out_ms {
        Some(out) => (i64::from(out), ms_to_frame(out)),
        None => (-1, 0),
    };
    let now = timestamp();
    // MPEG frame/offset columns stay 0: rekordbox fills them only for MP3s with a VBR seek table
    // (`MpegFrameOffset::GetMpegFrameOffset`), which musicbox does not compute yet (open).
    let columns: [(&str, Value); 15] = [
        ("ID", Value::Text(id.clone())),
        ("ContentID", Value::Text(content.0.clone())),
        ("InMsec", Value::Integer(i64::from(cue.in_ms))),
        ("InFrame", Value::Integer(ms_to_frame(cue.in_ms))),
        ("InMpegFrame", Value::Integer(0)),
        ("InMpegAbs", Value::Integer(0)),
        ("OutMsec", Value::Integer(out_msec)),
        ("OutFrame", Value::Integer(out_frame)),
        ("OutMpegFrame", Value::Integer(0)),
        ("OutMpegAbs", Value::Integer(0)),
        ("Kind", Value::Integer(cue.slot.kind())),
        ("Color", Value::Integer(-1)),
        ("UUID", Value::Text(new_uuid())),
        ("created_at", Value::Text(now.clone())),
        ("updated_at", Value::Text(now)),
    ];
    let names = columns.iter().map(|(c, _)| *c).collect::<Vec<_>>().join(", ");
    let marks = (1..=columns.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    tx.connection().execute(
        &format!("INSERT INTO djmdCue ({names}) VALUES ({marks})"),
        params_from_iter(columns.iter().map(|(_, v)| v.clone())),
    )?;
    library_sync(&tx, content.as_str())?;
    // `addCueOption(uint, analyzer::Cue const&)` @0x100a65658 → @0x100a6573c: all six option
    // columns at once. The trigger is a colour, a comment or a colour code (an active-loop flag
    // alone does not trigger it).
    if cue.color.is_some() || !cue.comment.is_empty() || cue.color_table_index.is_some_and(|c| c != 0) {
        update_cue(
            &tx,
            &id,
            content.as_str(),
            &[
                ("Color", Value::Integer(i64::from(cue.color.unwrap_or(0xFF)))),
                ("ColorTableIndex", Value::Integer(i64::from(cue.color_table_index.unwrap_or(0)))),
                ("ActiveLoop", Value::Integer(0)),
                ("Comment", Value::Text(cue.comment.clone())),
                ("BeatLoopSize", Value::Integer(0)),
                ("CueMicrosec", Value::Integer(0)),
            ],
        )?;
    }
    finish(tx, content.as_str())?;
    Ok((Effect::CueAdded { id: id.clone() }, Some(Command::DeleteCue { id })))
}

/// `rekordboxDBController::deleteCue` @0x1017de7ac (type 3): `AppSyncDBController::deleteCue` →
/// `CueRepository::DeleteById` (plain DELETE by ID, no USN, then library sync), then
/// `updateCueUpdated`. The inverse re-inserts the identical row.
pub(crate) fn delete_cue(lib: &mut Library, id: &str) -> Result<(Effect, Option<Command>)> {
    let row = cue_row(lib, id)?;
    let content = text(get(&row, "ContentID")).unwrap_or_default();
    let tx = lib.writer.transaction()?;
    tx.connection().execute("DELETE FROM djmdCue WHERE ID = ?1", [id])?;
    library_sync(&tx, &content)?;
    finish(tx, &content)?;
    touch_playlists(lib, &content)?;
    Ok((Effect::Done, Some(Command::RestoreCue { row })))
}

/// Undo of [`delete_cue`]: the old row goes back unchanged (same ID, UUID and timestamps), then
/// library sync and `CueUpdated` as for any cue change. Not a rekordbox operation.
pub(crate) fn restore_cue(lib: &mut Library, row: &RawRow) -> Result<(Effect, Option<Command>)> {
    let id = text(get(row, "ID")).ok_or_else(|| Error::Invalid("cue row without ID".into()))?;
    let content = text(get(row, "ContentID")).unwrap_or_default();
    require_track(lib, &content)?;
    let tx = lib.writer.transaction()?;
    let names = row.iter().map(|(c, _)| format!("\"{c}\"")).collect::<Vec<_>>().join(", ");
    let marks = (1..=row.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    tx.connection()
        .execute(&format!("INSERT INTO djmdCue ({names}) VALUES ({marks})"), params_from_iter(row.iter().map(|(_, v)| v.clone())))?;
    library_sync(&tx, &content)?;
    finish(tx, &content)?;
    touch_playlists(lib, &content)?;
    Ok((Effect::CueAdded { id: id.clone() }, Some(Command::DeleteCue { id })))
}

/// `rekordboxDBController::modifyCueColor` @0x1017ee19c and siblings (type 3): one ORM update of
/// the cue (+ library sync), then `updateCueUpdated`.
pub(crate) fn edit_cue(lib: &mut Library, id: &str, edit: &CueEdit) -> Result<(Effect, Option<Command>)> {
    let row = cue_row(lib, id)?;
    let content = text(get(&row, "ContentID")).unwrap_or_default();
    let (column, value, inverse) = match edit {
        CueEdit::Color(c) => {
            let old = int(get(&row, "Color")).filter(|c| (0..=127).contains(c)).map(|c| c as u8);
            ("Color", Value::Integer(i64::from(c.unwrap_or(0xFF))), CueEdit::Color(old))
        }
        CueEdit::ColorTableIndex(c) => {
            let old = int(get(&row, "ColorTableIndex")).unwrap_or(0).clamp(0, 255) as u8;
            ("ColorTableIndex", Value::Integer(i64::from(*c)), CueEdit::ColorTableIndex(old))
        }
        CueEdit::Comment(s) => ("Comment", Value::Text(s.clone()), CueEdit::Comment(text(get(&row, "Comment")).unwrap_or_default())),
        CueEdit::ActiveLoop(on) => {
            let old = int(get(&row, "ActiveLoop")).is_some_and(|a| a & 1 != 0);
            ("ActiveLoop", Value::Integer(i64::from(*on)), CueEdit::ActiveLoop(old))
        }
    };
    if &inverse == edit {
        return Ok((Effect::Unchanged, None));
    }
    let tx = lib.writer.transaction()?;
    update_cue(&tx, id, &content, &[(column, value)])?;
    finish(tx, &content)?;
    touch_playlists(lib, &content)?;
    Ok((Effect::Done, Some(Command::EditCue { id: id.to_owned(), edit: inverse })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_follow_set_hot_cue_data() {
        assert_eq!(CueSlot::Memory.kind(), 0);
        assert_eq!(CueSlot::Hot(0).kind(), 1);
        assert_eq!(CueSlot::Hot(2).kind(), 3);
        assert_eq!(CueSlot::Hot(3).kind(), 5);
        assert_eq!(CueSlot::Hot(7).kind(), 9);
        for k in [0, 1, 2, 3, 5, 9, 17] {
            assert_eq!(CueSlot::from_kind(k).unwrap().kind(), k);
        }
        assert_eq!(CueSlot::from_kind(4), Some(CueSlot::Memory));
        assert_eq!(CueSlot::from_kind(18), None);
        assert_eq!(CueSlot::Hot(7).letter(), Some('H'));
    }

    #[test]
    fn frames_truncate_like_c() {
        assert_eq!(ms_to_frame(0), 0);
        assert_eq!(ms_to_frame(20), 3);
        assert_eq!(ms_to_frame(1001), 150);
        assert_eq!(ms_to_frame(123_456), 18_518);
    }

    #[test]
    fn json_matches_entity_serialisation() {
        let row: RawRow = vec![
            ("ID".into(), Value::Text("17".into())),
            ("ContentID".into(), Value::Text("42".into())),
            ("InMsec".into(), Value::Integer(1000)),
            ("OutMsec".into(), Value::Integer(-1)),
            ("Kind".into(), Value::Integer(1)),
            ("Comment".into(), Value::Text("drop".into())),
            ("InPointSeekInfo".into(), Value::Null),
            ("UUID".into(), Value::Text("u".into())),
        ];
        assert_eq!(
            cues_json(&[row]),
            r#"[{"ID":"17","ContentID":"42","InMsec":1000,"OutMsec":-1,"Kind":1,"Comment":"drop","UUID":"u"}]"#
        );
        assert_eq!(cues_json(&[]), "[]");
        assert_eq!(juce_quoted(""), "\"\"");
        assert_eq!(juce_quoted("\"a"), "\"a\"");
    }
}
