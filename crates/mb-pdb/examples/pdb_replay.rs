//! Replays rekordbox's export history of the oracle stick (`fixtures/oracle-a/rbtest`) through
//! `mb_pdb::writer::Exporter` and compares the result with rekordbox's file page by page.
//! Usage: `cargo run -p mb-pdb --example pdb_replay -- <rbtest PIONEER/rekordbox dir> [out_dir]`

use std::path::PathBuf;

use mb_pdb::{Pdb, compare, lint, replay};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1).map(PathBuf::from);
    let dir = args.next().ok_or("usage: pdb_replay <dir> [out_dir]")?;
    let out_dir = args.next();
    let src = std::fs::read(dir.join("export.pdb"))?;
    let ours = replay::oracle_export(&Pdb::parse(src.clone())?)?;
    show("export.pdb", &src, &ours);
    let src_ext = std::fs::read(dir.join("exportExt.pdb"))?;
    let ours_ext = replay::oracle_ext(&Pdb::parse(src_ext.clone())?)?;
    show("exportExt.pdb", &src_ext, &ours_ext);
    if let Some(o) = out_dir {
        std::fs::create_dir_all(&o)?;
        std::fs::write(o.join("export.pdb"), &ours)?;
        std::fs::write(o.join("exportExt.pdb"), &ours_ext)?;
    }
    Ok(())
}

fn show(name: &str, theirs: &[u8], ours: &[u8]) {
    let kind = lint::kind_of(ours).expect("kind");
    let rep = lint::lint(ours, kind);
    println!(
        "{name}: lint {} errors / {} warnings",
        rep.errors(),
        rep.warnings()
    );
    for f in &rep.findings {
        println!("  {f}");
    }
    print!("{name}: {}", compare::compare(theirs, ours));
}
