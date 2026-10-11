//! `cargo run -p mb-masterdb --example schema -- <options.json> <master.db>`
//! Prints the full schema (tables, then indexes) as SQL.

use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let paths = LibraryPaths::from_options_json(&PathBuf::from(args.next().expect("options.json")))?;
    let db_path = PathBuf::from(args.next().expect("master.db"));
    let db = MasterDb::open_read_only(&db_path, &derive_passphrase(&paths.dp)?)?;
    let mut stmt = db.connection().prepare(
        "SELECT sql FROM sqlite_master WHERE sql IS NOT NULL \
         ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, tbl_name, name",
    )?;
    for sql in stmt.query_map([], |row| row.get::<_, String>(0))? {
        println!("{};", sql?);
    }
    Ok(())
}
