//! `cargo run -p mb-masterdb --example dump -- <options.json> <master.db> <out_dir>`
//! Writes every table as `<out_dir>/<table>.tsv`, rows sorted, so two dumps can be compared with
//! `diff -r` (before/after checks of what rekordbox or musicbox changed). Blobs are shown as
//! length + MD5. The agent's credential row is redacted. Read-only; development aid.

use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use md5::{Digest, Md5};
use std::io::Write;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let options = PathBuf::from(args.next().expect("options.json"));
    let db_path = PathBuf::from(args.next().expect("master.db"));
    let out = PathBuf::from(args.next().expect("out_dir"));
    std::fs::create_dir_all(&out)?;
    let paths = LibraryPaths::from_options_json(&options)?;
    let db = MasterDb::open_read_only(&db_path, &derive_passphrase(&paths.dp)?)?;
    let conn = db.connection();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for table in tables {
        let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\""))?;
        let n = stmt.column_count();
        let header = stmt.column_names().join("\t");
        let mut lines: Vec<String> = stmt
            .query_map([], |row| {
                Ok((0..n)
                    .map(|i| match row.get::<_, Value>(i) {
                        Ok(Value::Null) => "NULL".into(),
                        Ok(Value::Integer(v)) => v.to_string(),
                        Ok(Value::Real(v)) => v.to_string(),
                        Ok(Value::Text(s)) => s.replace(['\t', '\n'], " "),
                        Ok(Value::Blob(b)) => format!("<{} bytes md5 {:x}>", b.len(), Md5::digest(&b)),
                        Err(e) => format!("<{e}>"),
                    })
                    .collect::<Vec<_>>()
                    .join("\t"))
            })?
            .collect::<Result<_, _>>()?;
        if table == "agentRegistry" {
            for l in &mut lines {
                if l.starts_with("agentCredentials\t") {
                    *l = "agentCredentials\t<redacted>".into();
                }
            }
        }
        lines.sort();
        let mut f = std::fs::File::create(out.join(format!("{table}.tsv")))?;
        writeln!(f, "{header}")?;
        for l in &lines {
            writeln!(f, "{l}")?;
        }
        println!("{table}\t{}", lines.len());
    }
    Ok(())
}
