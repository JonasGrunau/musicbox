//! `cargo run -p mb-devlib --example devlib_rebuild -- <library_dir> <device_root> <out.db>`
//!
//! Rebuilds `<device_root>/PIONEER/rekordbox/exportLibrary.db` from the library in
//! `<library_dir>` (`options.json` + `rekordbox/master.db`) with `DevLibWriter`, writes it to
//! `<out.db>` and compares the two table by table, row by row, column by column. Neither input
//! is opened in place: master.db is copied to a temp dir, the stick's database is opened with
//! `immutable=1`. Example: `fixtures/oracle-a fixtures/oracle-a/rbtest /tmp/rebuilt.db`.

mod rebuild;

use mb_devlib::DevLibDb;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [library_dir, device_root, out] = args.as_slice() else {
        anyhow::bail!("usage: devlib_rebuild <library_dir> <device_root> <out.db>");
    };
    let summary = rebuild::rebuild(library_dir, device_root, out)?;
    for (name, inserted, updated, skipped) in &summary.exports {
        println!(
            "export {name}: {inserted} inserted, {updated} updated in place, {skipped} refused"
        );
    }
    for (id, file) in &summary.refused {
        println!("  refused by the format gate: {id} {file}");
    }
    println!("myTagMasterDBID {}", summary.my_tag_master_db_id);
    let side_files: Vec<_> = ["-wal", "-shm"]
        .iter()
        .filter(|s| PathBuf::from(format!("{}{s}", out.display())).exists())
        .collect();
    println!("closed cleanly (no -wal/-shm): {}", side_files.is_empty());

    // Compare copies opened normally (not `immutable=1`, which hides the WAL journal mode).
    let tmp = rebuild::TempDir::new("rebuild-compare")?;
    let (ours_path, theirs_path) = (tmp.0.join("ours.db"), tmp.0.join("theirs.db"));
    std::fs::copy(out, &ours_path)?;
    std::fs::copy(
        device_root.join("PIONEER/rekordbox/exportLibrary.db"),
        &theirs_path,
    )?;
    let ours = DevLibDb::open_read_only(&ours_path)?;
    let theirs = DevLibDb::open_read_only(&theirs_path)?;
    let cmp = mb_devlib::diff::compare(ours.connection(), theirs.connection())?;
    println!("left = {}, right = rekordbox", out.display());
    print!("{cmp}");
    println!(
        "content identical: {}",
        if cmp.content_identical() { "yes" } else { "no" }
    );
    Ok(())
}
