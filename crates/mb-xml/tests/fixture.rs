//! Round trip of the rekordbox 7.2.19 export in fixtures/oracle-a (skipped when fixtures are absent).

use mb_xml::{Document, Element, KeyType, PlaylistNode};
use std::path::PathBuf;

fn fixture() -> Option<String> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-a/rekordbox.xml");
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(_) => {
            eprintln!("no fixture at {}, skipping", path.display());
            None
        }
    }
}

/// Points at the first differing byte, for a readable failure.
fn assert_same_bytes(actual: &str, expected: &str) {
    if actual != expected {
        let at = actual
            .bytes()
            .zip(expected.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(actual.len().min(expected.len()));
        let from = at.saturating_sub(80);
        panic!(
            "output differs at byte {at} (lengths {} vs {}):\n--- ours\n{:?}\n--- rekordbox\n{:?}",
            actual.len(),
            expected.len(),
            &actual[from..(at + 80).min(actual.len())],
            &expected[from..(at + 80).min(expected.len())]
        );
    }
}

#[test]
fn tree_writer_is_byte_exact() {
    let Some(xml) = fixture() else { return };
    assert_same_bytes(&Element::parse(&xml).unwrap().to_document(), &xml);
}

#[test]
fn typed_round_trip_is_byte_exact() {
    let Some(xml) = fixture() else { return };
    let doc = Document::parse(&xml).unwrap();
    let written = doc.to_xml();
    assert_same_bytes(&written, &xml);
    assert_eq!(Document::parse(&written).unwrap(), doc);
}

#[test]
fn typed_content() {
    let Some(xml) = fixture() else { return };
    let doc = Document::parse(&xml).unwrap();
    assert_eq!(doc.version.as_deref(), Some("1.0.0"));
    let product = doc.product.as_ref().unwrap();
    assert_eq!(
        (
            product.name.as_deref(),
            product.version.as_deref(),
            product.company.as_deref()
        ),
        (Some("rekordbox"), Some("7.2.19"), Some("AlphaTheta"))
    );
    let tracks = doc.tracks();
    assert_eq!(tracks.len(), 161);
    assert_eq!(tracks.iter().map(|t| t.tempos.len()).sum::<usize>(), 242);
    assert!(
        tracks
            .iter()
            .all(|t| t.position_marks.is_empty() && t.extra.is_empty())
    );
    let first = &tracks[0];
    assert_eq!(first.track_id, Some(89904879));
    assert_eq!(first.kind.as_deref(), Some("ALAC-Datei"));
    assert_eq!(first.average_bpm, Some(120.0));
    assert_eq!(first.tempos[0].inizio, 0.019);
    assert_eq!(
        mb_xml::export::location_from_uri(first.location.as_deref().unwrap()).as_deref(),
        Some("/Users/jonasgrunau/re-work/corpus/probes/m4a_alac/chords_Am.m4a")
    );

    let roots = doc.playlists.as_ref().unwrap();
    let [PlaylistNode::Folder { name, children, .. }] = roots.as_slice() else {
        panic!("one ROOT folder")
    };
    assert_eq!(name, "ROOT");
    let lists: Vec<(&str, usize)> = children
        .iter()
        .map(|n| match n {
            PlaylistNode::Playlist {
                name,
                key_type,
                entries,
                ..
            } => {
                assert_eq!(*key_type, Some(KeyType::TrackId));
                (name.as_str(), entries.len())
            }
            PlaylistNode::Folder { .. } => panic!("no folders in the fixture"),
        })
        .collect();
    assert_eq!(
        lists,
        [
            ("oracle-small", 5),
            ("oracle-all", 161),
            ("CUE-Analyse Wiedergabeliste", 0)
        ]
    );
}

#[test]
fn unknown_content_survives() {
    let xml = "<DJ_PLAYLISTS Version=\"1.0.0\" X=\"1\"><COLLECTION Entries=\"1\"><TRACK TrackID=\"5\" Foo=\"bar\"><TEMPO Inizio=\"0.0435\" Bpm=\"128\" Metro=\"4/4\" Battito=\"1\"/><POSITION_MARK Name=\"a\" Type=\"4\" Start=\"1\" End=\"2.5\" Num=\"3\" Red=\"1\" Green=\"2\" Blue=\"3\"/><WHAT/></TRACK></COLLECTION><PLAYLISTS><NODE Type=\"0\" Name=\"ROOT\" Count=\"1\"><NODE Name=\"f\" Type=\"0\" Count=\"1\"><NODE Name=\"p\" Type=\"1\" KeyType=\"1\" Entries=\"1\"><TRACK Key=\"file://localhost/a\"/></NODE></NODE></NODE></PLAYLISTS></DJ_PLAYLISTS>";
    let doc = Document::parse(xml).unwrap();
    assert_eq!(doc.extra.attributes, [("X".to_owned(), "1".to_owned())]);
    let t = &doc.tracks()[0];
    assert_eq!(t.extra.attributes, [("Foo".to_owned(), "bar".to_owned())]);
    assert_eq!(t.extra.children.len(), 1);
    assert_eq!(t.position_marks[0].rgb, Some([1, 2, 3]));
    let out = doc.to_xml();
    // 0.0435 is 0.04349999… in binary, so %.03f gives 0.043, as in C.
    assert!(
        out.contains("<TEMPO Inizio=\"0.043\" Bpm=\"128.00\" Metro=\"4/4\" Battito=\"1\"/>"),
        "{out}"
    );
    assert!(
        out.contains("Start=\"1.000\" End=\"2.500\" Num=\"3\""),
        "{out}"
    );
    assert!(
        out.contains("<NODE Name=\"p\" Type=\"1\" KeyType=\"1\" Entries=\"1\">"),
        "{out}"
    );
    let again = Document::parse(&out).unwrap();
    assert_eq!(again.tracks()[0].extra, t.extra);
    assert_eq!(again.playlists, doc.playlists);
}
