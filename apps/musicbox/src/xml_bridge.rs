//! rekordbox.xml in the app: the "rekordbox xml" library in the tree (rekordbox's bridge,
//! `DjPlaylistsBridge::initialize` reads the file named by the preference
//! `bridgeImportedLibraryFile`) and "File → Export Collection in xml format" (`docs/re/xml.md`,
//! `docs/re/ops/import-xml.md`).

use mb_core::{DbId, FileType, LibrarySnapshot, PlaylistKind, PlaylistNode, Track};
use mb_library::xml::{XmlExportOptions, export_document, import_for_node, write_document};
use mb_library::{XmlImport, XmlList};
use mb_xml::export::{kind_from_label, location_from_uri};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

/// An opened rekordbox.xml, shown like a device library.
pub struct XmlLibrary {
    pub path: PathBuf,
    pub doc: mb_xml::Document,
    pub snapshot: LibrarySnapshot,
    /// Tree node → its position below `ROOT` (child indexes).
    nodes: HashMap<DbId, Vec<usize>>,
}

/// The file rekordbox's bridge shows (preference `bridgeImportedLibraryFile`), if the tree shows
/// it (preference `showRbXml` = 1; *hypothesis* from the name) and it exists. `pref` looks up a
/// preference (`library::Preferences`).
pub fn bridge_file(pref: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if pref("showRbXml")?.trim() != "1" {
        return None;
    }
    let path = PathBuf::from(pref("bridgeImportedLibraryFile")?);
    path.is_file().then_some(path)
}

pub fn open(path: &Path) -> Result<XmlLibrary, String> {
    let doc = mb_library::xml::read_document(path).map_err(|e| e.to_string())?;
    let mut snapshot = LibrarySnapshot::default();
    for (i, x) in doc.tracks().iter().enumerate() {
        snapshot.tracks.push(track(i, x));
    }
    let mut nodes = HashMap::new();
    if let Some([mb_xml::PlaylistNode::Folder { children, .. }]) = doc.playlists.as_deref() {
        let mut next = 0usize;
        add_nodes(&doc, children, None, &mut Vec::new(), &mut next, &mut snapshot, &mut nodes);
    }
    Ok(XmlLibrary { path: path.to_path_buf(), doc, snapshot, nodes })
}

/// A collection row for the browser. The ID is the XML `TrackID` (as rekordbox shows it).
fn track(index: usize, x: &mb_xml::Track) -> Track {
    let s = |v: &Option<String>| v.clone().unwrap_or_default();
    let path = PathBuf::from(x.location.as_deref().and_then(location_from_uri).unwrap_or_default());
    Track {
        id: DbId(x.track_id.map_or_else(|| format!("#{index}"), |id| id.to_string())),
        title: s(&x.name),
        artist: s(&x.artist),
        album: s(&x.album),
        genre: s(&x.genre),
        label: s(&x.label),
        remixer: s(&x.remixer),
        composer: s(&x.composer),
        original_artist: String::new(),
        album_artist: String::new(),
        mix_name: s(&x.mix),
        file_name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        key: s(&x.tonality),
        color: x.colour.and_then(|c| mb_xml::import::colour_id_from_text(&format!("0x{c:06X}"))),
        bpm_x100: x.average_bpm.map(mb_xml::import::bpm_x100_from_xml),
        length_seconds: x.total_time,
        track_no: x.track_number,
        disc_no: x.disc_number,
        release_year: x.year,
        rating: mb_xml::export::rating_from_xml(i64::from(x.rating.unwrap_or(0))).min(5),
        comment: s(&x.comments),
        play_count: x.play_count.unwrap_or(0),
        bitrate: x.bit_rate,
        sample_rate: x.sample_rate,
        bit_depth: None,
        file_size: x.size.map(|v| v as u64),
        file_type: x.kind.as_deref().and_then(kind_from_label).map_or(FileType::Other(0), |(code, _)| FileType::from_code(code)),
        path,
        date_added: s(&x.date_added),
        date_created: String::new(),
        release_date: String::new(),
        analysis_path: None,
        artwork_path: None,
        analysed: 0,
    }
}

fn add_nodes(
    doc: &mb_xml::Document,
    nodes: &[mb_xml::PlaylistNode],
    parent: Option<&DbId>,
    path: &mut Vec<usize>,
    next: &mut usize,
    snapshot: &mut LibrarySnapshot,
    index: &mut HashMap<DbId, Vec<usize>>,
) {
    for (i, n) in nodes.iter().enumerate() {
        let id = DbId(format!("xml{next}"));
        *next += 1;
        path.push(i);
        index.insert(id.clone(), path.clone());
        let kind = match n {
            mb_xml::PlaylistNode::Folder { .. } => PlaylistKind::Folder,
            mb_xml::PlaylistNode::Playlist { .. } => PlaylistKind::Playlist,
        };
        snapshot.playlists.push(PlaylistNode { id: id.clone(), parent: parent.cloned(), name: n.name().to_owned(), kind, seq: i as i64 });
        match n {
            mb_xml::PlaylistNode::Folder { children, .. } => add_nodes(doc, children, Some(&id), path, next, snapshot, index),
            mb_xml::PlaylistNode::Playlist { entries, key_type, .. } => {
                let tracks = mb_library::xml::playlist_tracks(doc, entries, *key_type);
                let ids = tracks
                    .iter()
                    .map(|t| DbId(t.track_id.map_or_else(String::new, |id| id.to_string())))
                    .collect();
                snapshot.playlist_entries.insert(id, ids);
            }
        }
        path.pop();
    }
}

impl XmlLibrary {
    /// The XML node behind a tree ID.
    fn node(&self, id: &DbId) -> Option<&mb_xml::PlaylistNode> {
        let path = self.nodes.get(id)?;
        let mut nodes = match self.doc.playlists.as_deref() {
            Some([mb_xml::PlaylistNode::Folder { children, .. }]) => children.as_slice(),
            _ => return None,
        };
        let mut found = None;
        for &i in path {
            let n = nodes.get(i)?;
            found = Some(n);
            nodes = match n {
                mb_xml::PlaylistNode::Folder { children, .. } => children,
                mb_xml::PlaylistNode::Playlist { .. } => &[],
            };
        }
        found
    }

    /// Importing one playlist or folder: its tracks and the list tree.
    pub fn import_for_list(&self, id: &DbId) -> Option<XmlImport> {
        Some(import_for_node(&self.doc, self.node(id)?))
    }

    /// Importing single tracks (rows of the XML collection), no list.
    pub fn import_for_tracks(&self, rows: &[usize]) -> XmlImport {
        let tracks = rows.iter().filter_map(|&i| self.doc.tracks().get(i).cloned()).collect();
        XmlImport { tracks, ..XmlImport::default() }
    }
}

/// How many of `import`'s tracks are already in `local` (same file path).
pub fn already_in_collection(import: &XmlImport, local: &LibrarySnapshot) -> usize {
    let paths: std::collections::HashSet<&Path> = local.tracks.iter().map(|t| t.path.as_path()).collect();
    import
        .tracks
        .iter()
        .filter(|t| t.location.as_deref().and_then(location_from_uri).is_some_and(|p| paths.contains(Path::new(&p))))
        .count()
}

/// Names of `import`'s top-level lists that already exist at the top of `local`'s Playlists.
pub fn same_name_lists(import: &XmlImport, local: &LibrarySnapshot) -> Vec<String> {
    import
        .lists
        .iter()
        .map(XmlList::name)
        .filter(|name| local.playlists.iter().any(|n| n.parent.is_none() && n.name == *name))
        .map(str::to_owned)
        .collect()
}

/// A rekordbox.xml export running in the background.
pub struct XmlExportJob {
    pub path: PathBuf,
    rx: Receiver<Result<String, String>>,
}

impl XmlExportJob {
    /// `snapshot` is shared with the app (no copy of the library on the UI thread).
    pub fn start(ctx: &egui::Context, source: &crate::export::Source, snapshot: std::sync::Arc<LibrarySnapshot>, path: PathBuf) -> Self {
        let (tx, rx) = channel();
        let (db, pass, settings, out) = (source.db_path.clone(), source.passphrase.clone(), source.settings_root.clone(), path.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                let master = mb_masterdb::MasterDb::open_read_only(&db, &pass).map_err(|e| e.to_string())?;
                let opts = XmlExportOptions::from_settings(&settings);
                let doc = export_document(master.connection(), &snapshot, &opts).map_err(|e| e.to_string())?;
                write_document(&doc, &out).map_err(|e| e.to_string())?;
                Ok(format!("Exported {} tracks to {}", doc.tracks().len(), out.display()))
            })();
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Self { path, rx }
    }

    pub fn poll(&mut self) -> Option<Result<String, String>> {
        self.rx.try_recv().ok()
    }
}

/// rekordbox's default export location is not known; musicbox offers `~/Documents/rekordbox.xml`.
pub fn default_export_path() -> String {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Documents/rekordbox.xml").display().to_string()).unwrap_or_default()
}
