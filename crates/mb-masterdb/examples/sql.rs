//! `cargo run -p mb-masterdb --example sql -- <options.json> <master.db> "<SELECT …>"`
//! Runs one read-only query and prints the rows tab-separated (development aid).

use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let options = PathBuf::from(args.next().expect("options.json"));
    let db_path = PathBuf::from(args.next().expect("master.db"));
    let sql = args.next().expect("SQL");
    let paths = LibraryPaths::from_options_json(&options)?;
    let db = MasterDb::open_read_only(&db_path, &derive_passphrase(&paths.dp)?)?;
    let mut stmt = db.connection().prepare(&sql)?;
    let n = stmt.column_count();
    println!("{}", stmt.column_names().join("\t"));
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let cells: Vec<String> = (0..n)
            .map(|i| match row.get::<_, Value>(i) {
                Ok(Value::Null) => "NULL".into(),
                Ok(Value::Integer(v)) => v.to_string(),
                Ok(Value::Real(v)) => v.to_string(),
                Ok(Value::Text(s)) => s,
                Ok(Value::Blob(b)) => format!("<{} bytes>", b.len()),
                Err(e) => format!("<{e}>"),
            })
            .collect();
        println!("{}", cells.join("\t"));
    }
    Ok(())
}
