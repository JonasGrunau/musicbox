//! M2 acceptance check: applies a fixed set of edits to a library so the user can confirm in
//! rekordbox that they show up correctly. Refuses while rekordbox runs; backs up first.
//!
//! `cargo run -p mb-library --example m2_check -- <options.json> <import_file>`

use mb_core::DbId;
use mb_library::{Command, Effect, Library, NodeKind, TrackEdit};
use mb_masterdb::{LibraryPaths, MasterDb, backup_library, default_backup_root, derive_passphrase};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let options = PathBuf::from(args.next().expect("options.json"));
    let import_file = PathBuf::from(args.next().expect("file to import"));
    let paths = LibraryPaths::from_options_json(&options)?;
    let pass = derive_passphrase(&paths.dp)?;

    let mut lib = Library::open(&paths.db_path, &pass)?; // refuses while rekordbox runs
    let backup = backup_library(&paths.db_path, &pass, &default_backup_root().expect("HOME"))?;
    println!("backup: {}", backup.display());

    let snap = MasterDb::open_read_only(&paths.db_path, &pass)?.load_snapshot(&paths)?;
    let real: Vec<&mb_core::Track> = snap.tracks.iter().filter(|t| !t.artist.is_empty()).take(5).collect();
    let ids: Vec<DbId> = real.iter().map(|t| t.id.clone()).collect();
    let created = |e: &Effect| match e {
        Effect::PlaylistCreated { id } => id.clone(),
        other => panic!("unexpected {other:?}"),
    };

    let folder = created(&lib.create_playlist(None, "musicbox folder", NodeKind::Folder)?.effect);
    let list = created(&lib.create_playlist(Some(&folder), "musicbox check", NodeKind::Playlist)?.effect);
    lib.add_tracks(&list, &ids)?;
    if let Some(small) = snap.playlists.iter().find(|p| p.name == "oracle-small") {
        lib.execute(Command::RenamePlaylist { id: small.id.clone(), name: "oracle-small (renamed by musicbox)".into() })?;
    }
    for id in &ids {
        lib.edit_track(id, TrackEdit::Rating(3))?;
        lib.edit_track(id, TrackEdit::Color(5))?;
    }
    let first = real[0];
    lib.edit_track(&first.id, TrackEdit::Title(format!("{} [edited by musicbox]", first.title)))?;
    let imported = lib.import_files(std::slice::from_ref(&import_file))?;
    println!("import: {:?}", imported.effect);

    println!("\nExpected in rekordbox:");
    println!("1. Playlists: folder \"musicbox folder\" containing \"musicbox check\" with these 5 tracks, in this order:");
    for t in &real {
        println!("     - {} — {}", t.artist, t.title);
    }
    println!("2. \"oracle-small\" is renamed to \"oracle-small (renamed by musicbox)\".");
    println!("3. Those 5 tracks: 3 stars, colour Green.");
    println!("4. Title of \"{}\" now ends with \" [edited by musicbox]\".", first.title);
    println!("5. New track in the collection: {}", import_file.display());
    println!("6. No warning/repair dialog when rekordbox opens the library.");
    Ok(())
}
