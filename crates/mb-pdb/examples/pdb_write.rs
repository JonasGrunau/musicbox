//! Reads export.pdb + exportExt.pdb, writes them again with the writer, and checks the result.
//! Usage: `cargo run -p mb-pdb --example pdb_write -- <in_dir> <out_dir> [compact|export]`
//! `<in_dir>` is a `PIONEER/rekordbox` directory. `compact` (default) writes every table in one
//! transaction (`mb_pdb::writer::write_export`); `export` replays rekordbox's per-track export
//! sequence for a fresh stick (`mb_pdb::writer::export_like_rekordbox`). The example reads the
//! output back, compares the rows, lints it and prints a page-by-page comparison with the input.

use std::path::PathBuf;

use mb_pdb::library::{ExportDb, ExtDb};
use mb_pdb::schema::DbKind;
use mb_pdb::{Pdb, compare, lint, writer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1).map(PathBuf::from);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        return Err("usage: pdb_write <in_dir> <out_dir>".into());
    };
    let export_mode = match args.next().as_deref().and_then(|a| a.to_str()) {
        None | Some("compact") => false,
        Some("export") => true,
        Some(m) => return Err(format!("unknown mode {m}").into()),
    };
    std::fs::create_dir_all(&output)?;

    let src = std::fs::read(input.join("export.pdb"))?;
    let rows = ExportDb::read(&Pdb::parse(src.clone())?)?;
    let out = if export_mode {
        writer::export_like_rekordbox(&rows)?
    } else {
        writer::write_export(&rows)?
    };
    std::fs::write(output.join("export.pdb"), &out)?;
    let back = ExportDb::read(&Pdb::parse(out.clone())?)?;
    report(
        "export.pdb",
        &src,
        &out,
        same_rows(&rows, &back),
        DbKind::Export,
    );

    let ext_path = input.join("exportExt.pdb");
    if ext_path.exists() {
        let src = std::fs::read(&ext_path)?;
        let rows = ExtDb::read(&Pdb::parse(src.clone())?)?;
        let out = if export_mode {
            writer::ext_like_rekordbox(&rows)?
        } else {
            writer::write_ext(&rows)?
        };
        std::fs::write(output.join("exportExt.pdb"), &out)?;
        let back = ExtDb::read(&Pdb::parse(out.clone())?)?;
        report(
            "exportExt.pdb",
            &src,
            &out,
            same_ext_rows(&rows, &back),
            DbKind::ExportExt,
        );
    }
    Ok(())
}

fn report(name: &str, src: &[u8], out: &[u8], same_rows: bool, kind: DbKind) {
    let rep = lint::lint(out, kind);
    println!(
        "{name}: {} bytes written, rows read back {}, lint {} errors / {} warnings",
        out.len(),
        if same_rows { "identical" } else { "DIFFERENT" },
        rep.errors(),
        rep.warnings()
    );
    for f in &rep.findings {
        println!("  {f}");
    }
    let c = compare::compare(src, out);
    println!(
        "  vs input: {} pages, {} identical, {} differ",
        c.pages,
        c.identical,
        c.diffs.len()
    );
}

/// Same rows, ignoring row order (the export sequence inserts in its own order).
fn same_rows(a: &ExportDb, b: &ExportDb) -> bool {
    sorted(a) == sorted(b)
}

fn same_ext_rows(a: &ExtDb, b: &ExtDb) -> bool {
    let key = |x: &ExtDb| {
        let mut v: Vec<String> = format!("{x:#?}").lines().map(str::to_owned).collect();
        v.sort();
        v
    };
    a == b || key(a) == key(b)
}

fn sorted(x: &ExportDb) -> Vec<String> {
    let mut v = vec![
        sorted_dbg(&x.tracks),
        sorted_dbg(&x.genres),
        sorted_dbg(&x.artists),
        sorted_dbg(&x.albums),
        sorted_dbg(&x.labels),
        sorted_dbg(&x.keys),
        sorted_dbg(&x.colors),
        sorted_dbg(&x.playlists),
        sorted_dbg(&x.playlist_entries),
        sorted_dbg(&x.histories),
        sorted_dbg(&x.history_entries),
        sorted_dbg(&x.artwork),
        sorted_dbg(&x.menu_items),
        sorted_dbg(&x.categories),
        sorted_dbg(&x.sorts),
        sorted_dbg(&x.properties),
    ];
    v.dedup();
    v
}

fn sorted_dbg<T: std::fmt::Debug>(rows: &[T]) -> String {
    let mut v: Vec<String> = rows.iter().map(|r| format!("{r:?}")).collect();
    v.sort();
    v.join("\n")
}
