//! M7 acceptance check: creates three intelligent playlists in the library, so the user can
//! confirm in rekordbox that they open in its editor with the same rules and show the same
//! tracks as musicbox. Refuses while rekordbox runs; backs the library up first.
//!
//! `cargo run -p mb-library --example m7_check -- <options.json>`

use mb_core::smartlist::{self, Condition, MATCH_ALL, MATCH_ANY, op};
use mb_library::{Command, Effect, Library};
use mb_masterdb::{LibraryPaths, MasterDb, backup_library, default_backup_root, derive_passphrase};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let options = PathBuf::from(std::env::args().nth(1).expect("options.json"));
    let paths = LibraryPaths::from_options_json(&options)?;
    let pass = derive_passphrase(&paths.dp)?;

    let mut lib = Library::open(&paths.db_path, &pass)?; // refuses while rekordbox runs
    let backup = backup_library(&paths.db_path, &pass, &default_backup_root().expect("HOME"))?;
    println!("backup: {}", backup.display());

    let lists: [(&str, i32, Vec<Condition>); 3] = [
        (
            "mb House 120-130",
            MATCH_ALL,
            vec![Condition::new("genre", op::CONTAINS, "", "house", ""), Condition::new("bpm", op::IN_RANGE, "", "120", "130")],
        ),
        (
            "mb Techno or Psy",
            MATCH_ANY,
            vec![Condition::new("genre", op::IS, "", "techno", ""), Condition::new("genre", op::STARTS_WITH, "", "psy", "")],
        ),
        ("mb Added last 30 days", MATCH_ALL, vec![Condition::new("stockDate", op::IN_LAST, "day", "30", "")]),
    ];
    let mut ids = Vec::new();
    for (name, logical, conditions) in lists {
        let out = lib.execute(Command::CreateSmartList { parent: None, name: name.into(), logical_operator: logical, conditions })?;
        let Effect::PlaylistCreated { id } = out.effect else { anyhow::bail!("not created: {:?}", out.effect) };
        ids.push((name, id));
    }

    let snap = MasterDb::open_read_only(&paths.db_path, &pass)?.load_snapshot(&paths)?;
    let results = smartlist::evaluate_all(&snap, smartlist::Clock::utc_now());
    println!("\nOpen rekordbox and check under Playlists (top of the tree):");
    for (name, id) in &ids {
        let tracks = results.get(id).map_or(0, Vec::len);
        println!("  {name:<24} {tracks:>4} tracks");
    }
    println!(
        "\nFor each list: the track count matches, and right-click → Edit Intelligent Playlist shows the same\n\
         rules (genre contains \"house\" + BPM 120–130 / any of: genre is \"techno\", genre starts with \"psy\" /\n\
         Date Added in the last 30 days). No warning or repair dialog on startup."
    );
    Ok(())
}
