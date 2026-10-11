//! `cargo run -p mb-devlib --example probe -- <exportLibrary.db>`
//! Row counts and the property row. Opens with `immutable=1`, so no side files are created.

use mb_devlib::DevLibDb;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let path = PathBuf::from(std::env::args().nth(1).expect("exportLibrary.db"));
    let db = DevLibDb::open_immutable(&path)?;
    for (table, count) in db.table_counts()? {
        println!("{table:20} {count}");
    }
    println!("{:?}", db.property()?);
    Ok(())
}
