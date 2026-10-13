//! "Analysis Lock On/Off" (`docs/re/ops/analyse-track.md` § Analysis lock):
//! `DatabaseMediator::modifyAnalysisLock` @0x100c4903c → `changeAnalysisLock` @0x1017d8b1c →
//! `AppSyncDBController::modAnalysisLockIfNeed` @0x100a5d2d8, which writes
//! `Analysed = (lock ? 0x80 : 0) | (Analysed & 0x7F)` through `updateAnalysed` @0x100a5d1c0 only
//! when the value changes. Locked tracks are not analysed and their grid cannot be edited.

use crate::{Command, Effect, Library, Result};
use mb_core::DbId;
use mb_masterdb::rusqlite::OptionalExtension;
use mb_masterdb::rusqlite::types::Value;

/// `Analysed` bit of the lock.
pub const LOCK: i64 = 0x80;

pub(crate) fn set_lock(lib: &mut Library, ids: &[DbId], locked: bool) -> Result<(Effect, Option<Command>)> {
    let tx = lib.writer.transaction()?;
    let mut changed = Vec::new();
    for id in ids {
        let analysed: Option<Option<i64>> = tx
            .connection()
            .query_row("SELECT Analysed FROM djmdContent WHERE ID = ?1 AND rb_local_deleted = 0", [id.as_str()], |r| r.get(0))
            .optional()?;
        let Some(analysed) = analysed.map(Option::unwrap_or_default) else { continue };
        let new = if locked { LOCK } else { 0 } | (analysed & 0x7F);
        if new != analysed {
            tx.update("djmdContent", "ID", Value::Text(id.0.clone()), &[("Analysed", Value::Integer(new))])?;
            changed.push(id.clone());
        }
    }
    tx.commit()?;
    let inverse = (!changed.is_empty()).then_some(Command::SetAnalysisLock { ids: changed, locked: !locked });
    Ok((Effect::Done, inverse))
}

/// True when the track's analysis is locked.
pub fn is_locked(lib: &Library, id: &DbId) -> Result<bool> {
    let analysed: Option<Option<i64>> = lib
        .connection()
        .query_row("SELECT Analysed FROM djmdContent WHERE ID = ?1 AND rb_local_deleted = 0", [id.as_str()], |r| r.get(0))
        .optional()?;
    Ok(analysed.flatten().is_some_and(|a| a & LOCK != 0))
}
