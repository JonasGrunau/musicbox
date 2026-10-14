//! `cargo run -p mb-devlib --example query -- <exportLibrary.db> "<sql>"`
//! Runs one read-only SQL statement and prints the rows tab-separated (NULL as `∅`, text quoted
//! when empty). Opens with `immutable=1`, so no side files are created. With `MB_WAL=1` (exactly) it opens
//! normally and reads the `-wal` too: only on a copy, never on a device, because SQLite then creates
//! `-wal`/`-shm` next to the file (and macOS `._` files on FAT32).

use mb_devlib::DevLibDb;
use rusqlite::types::ValueRef;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("exportLibrary.db"));
    let sql = args.next().expect("sql");
    let db = if std::env::var("MB_WAL").is_ok_and(|v| v == "1") { DevLibDb::open_read_only(&path)? } else { DevLibDb::open_immutable(&path)? };
    let mut stmt = db.connection().prepare(&sql)?;
    let n = stmt.column_count();
    println!("{}", stmt.column_names().join("\t"));
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let cells: Vec<String> = (0..n)
            .map(|i| match r.get_ref(i) {
                Ok(ValueRef::Null) => "∅".into(),
                Ok(ValueRef::Integer(v)) => v.to_string(),
                Ok(ValueRef::Real(v)) => v.to_string(),
                Ok(ValueRef::Text(b"")) => "''".into(),
                Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).into_owned(),
                Ok(ValueRef::Blob(b)) => format!("<blob {}>", b.len()),
                Err(e) => format!("<{e}>"),
            })
            .collect();
        println!("{}", cells.join("\t"));
    }
    Ok(())
}
