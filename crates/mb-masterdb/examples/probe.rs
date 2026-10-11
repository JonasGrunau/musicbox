//! `cargo run -p mb-masterdb --example probe -- <options.json> [master.db]`
//! Opens a master.db read-only and prints every table's row count.

use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let options = args
        .next()
        .map(PathBuf::from)
        .or_else(LibraryPaths::default_options_json)
        .expect("no options.json path");
    let paths = LibraryPaths::from_options_json(&options)?;
    let db_path = args.next().map(PathBuf::from).unwrap_or(paths.db_path.clone());
    let passphrase = derive_passphrase(&paths.dp)?;
    let db = MasterDb::open_read_only(&db_path, &passphrase)?;
    for (table, count) in db.table_counts()? {
        println!("{count:>8}  {table}");
    }
    Ok(())
}
