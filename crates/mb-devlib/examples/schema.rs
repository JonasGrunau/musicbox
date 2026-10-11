//! `cargo run -p mb-devlib --example schema -- <exportLibrary.db>`
//! Prints the full schema (tables, then indexes) as SQL, plus the header pragmas as comments.
//! Open a copy: even a read-only open of a WAL database may create `-shm`/`-wal` next to it.

use mb_devlib::DevLibDb;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let path = PathBuf::from(std::env::args().nth(1).expect("exportLibrary.db"));
    let db = DevLibDb::open_read_only(&path)?;
    let conn = db.connection();
    for pragma in [
        "page_size",
        "journal_mode",
        "user_version",
        "application_id",
        "auto_vacuum",
        "encoding",
    ] {
        let value: String = conn.query_row(&format!("PRAGMA {pragma}"), [], |r| {
            r.get::<_, rusqlite::types::Value>(0).map(|v| match v {
                rusqlite::types::Value::Integer(i) => i.to_string(),
                rusqlite::types::Value::Text(t) => t,
                other => format!("{other:?}"),
            })
        })?;
        println!("-- PRAGMA {pragma} = {value}");
    }
    let mut stmt = conn.prepare(
        "SELECT sql FROM sqlite_master WHERE sql IS NOT NULL \
         ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, tbl_name, name",
    )?;
    for sql in stmt.query_map([], |row| row.get::<_, String>(0))? {
        println!("{};", sql?);
    }
    Ok(())
}
