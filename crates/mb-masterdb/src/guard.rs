//! Write guard: master.db must never be written while rekordbox or its agent runs, because both
//! hold their own connections and caches (docs/re/masterdb.md § Journal and writers).

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

/// Process names of the two rekordbox writers.
pub const REKORDBOX_PROCESSES: &[&str] = &["rekordbox", "rekordboxAgent"];

/// Names of rekordbox processes currently running (empty when writing is safe).
pub fn running_rekordbox() -> Vec<String> {
    let mut system = System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()));
    system.refresh_processes(ProcessesToUpdate::All, true);
    let mut found: Vec<String> = system
        .processes()
        .values()
        .filter_map(|p| p.name().to_str())
        .filter(|name| REKORDBOX_PROCESSES.contains(name))
        .map(str::to_owned)
        .collect();
    found.sort();
    found.dedup();
    found
}
