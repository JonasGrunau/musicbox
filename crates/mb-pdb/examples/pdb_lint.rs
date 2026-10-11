//! Structural checks of export.pdb / exportExt.pdb (`mb_pdb::lint`).
//! Usage: `cargo run -p mb-pdb --example pdb_lint -- <file.pdb | PIONEER/rekordbox dir>...`
//! Exit status 1 when any file has errors.

use std::path::{Path, PathBuf};

fn files(arg: &Path) -> Vec<PathBuf> {
    if arg.is_dir() {
        ["export.pdb", "exportExt.pdb"]
            .iter()
            .map(|f| arg.join(f))
            .filter(|p| p.exists())
            .collect()
    } else {
        vec![arg.to_owned()]
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if args.is_empty() {
        return Err("usage: pdb_lint <file.pdb | dir>...".into());
    }
    let mut failed = false;
    for path in args.iter().flat_map(|a| files(a)) {
        let bytes = std::fs::read(&path)?;
        let Some(kind) = mb_pdb::lint::kind_of(&bytes) else {
            println!("{}: not an export.pdb/exportExt.pdb", path.display());
            failed = true;
            continue;
        };
        let rep = mb_pdb::lint::lint(&bytes, kind);
        println!(
            "{}: {} pages, {} rows checked: {} errors, {} warnings",
            path.display(),
            rep.pages_checked,
            rep.rows_checked,
            rep.errors(),
            rep.warnings()
        );
        for f in &rep.findings {
            println!("  {f}");
        }
        failed |= !rep.is_clean();
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}
