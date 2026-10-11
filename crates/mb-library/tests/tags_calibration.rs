//! Tag reading against rekordbox's own import of the corpus: for each of the 161 traced imports,
//! the values `mb_library::tags::read_file` derives must equal what rekordbox inserted.
//! Skipped without the oracle-a trace or the corpus (`~/re-work/corpus`).

mod common;

use common::*;
use mb_library::tags;
use std::collections::BTreeMap;

#[test]
fn tag_values_match_rekordbox_import() {
    let Some(trace) = trace() else {
        eprintln!("no trace, skipping");
        return;
    };
    let imports = traced_imports(&trace);
    assert_eq!(imports.len(), 161);
    if !imports[0].path().exists() {
        eprintln!("corpus missing, skipping");
        return;
    }
    let mut mismatches: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut checked = 0;
    for imp in &imports {
        let path = imp.path();
        let t = tags::read_file(&path).unwrap_or_else(|e| panic!("{e}"));
        checked += 1;
        let names = traced_names(imp);
        let checks: Vec<(&str, String, String)> = vec![
            ("Title", imp.text("Title").unwrap(), t.title.clone()),
            ("Commnt", imp.text("Commnt").unwrap(), t.comment.clone()),
            ("TrackNo", imp.text("TrackNo").unwrap(), t.track_no.to_string()),
            ("DiscNo", imp.text("DiscNo").unwrap(), t.disc_no.to_string()),
            ("ReleaseYear", imp.text("ReleaseYear").unwrap(), t.release_year.to_string()),
            ("ReleaseDate", imp.text("ReleaseDate").unwrap(), t.release_date.clone()),
            ("ISRC", imp.text("ISRC").unwrap(), t.isrc.clone()),
            ("Length", imp.text("Length").unwrap(), t.length.to_string()),
            ("FileType", imp.text("FileType").unwrap(), t.file_type.to_string()),
            ("FileSize", imp.text("FileSize").unwrap(), t.file_size.to_string()),
            ("rb_file_id", imp.text("rb_file_id").unwrap(), t.inode.to_string()),
            ("Lyricist", imp.text("Lyricist").unwrap(), t.lyricist.clone()),
            ("artist", names[0].clone(), t.artist.clone()),
            ("genre", names[1].clone(), t.genre.clone()),
            ("album artist", names[2].clone(), if t.album.is_empty() { String::new() } else { t.album_artist.clone() }),
            ("album", names[3].clone(), t.album.clone()),
            ("remixer", names[4].clone(), t.remixer.clone()),
            ("label", names[5].clone(), t.label.clone()),
            ("key", names[6].clone(), t.key.clone()),
        ];
        for (field, want, got) in checks {
            if want != got {
                mismatches.entry(field).or_default().push(format!("{}: rekordbox {want:?} ours {got:?}", path.display()));
            }
        }
        if !t.composer.is_empty() || !t.original_artist.is_empty() {
            mismatches.entry("composer/orig artist present").or_default().push(format!(
                "{}: composer {:?} orig {:?} ComposerID in row: {} OrgArtistID in row: {}",
                path.display(),
                t.composer,
                t.original_artist,
                imp.row.contains_key("ComposerID"),
                imp.row.contains_key("OrgArtistID")
            ));
        }
    }
    for (field, list) in &mismatches {
        eprintln!("== {field}: {} mismatches", list.len());
        for m in list.iter().take(6) {
            eprintln!("   {m}");
        }
    }
    eprintln!("{checked} files checked");
    if std::env::var_os("MB_CALIBRATE").is_none() {
        let real: Vec<_> = mismatches.keys().filter(|k| !k.starts_with("composer")).collect();
        assert!(real.is_empty(), "fields with mismatches: {real:?}");
    }
}
