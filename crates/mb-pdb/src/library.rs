//! A whole device library (export.pdb + exportExt.pdb) and its conversion to
//! [`mb_core::LibrarySnapshot`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mb_core::{DbId, FileType, LibrarySnapshot, PlaylistKind};

use crate::Result;
use crate::page::Pdb;
use crate::tables::*;

/// Every table of export.pdb, typed.
#[derive(Debug, Clone, Default)]
pub struct ExportDb {
    pub tracks: Vec<Track>,
    pub genres: Vec<Genre>,
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub labels: Vec<Label>,
    pub keys: Vec<Key>,
    pub colors: Vec<Color>,
    pub playlists: Vec<PlaylistNode>,
    pub playlist_entries: Vec<PlaylistEntry>,
    pub hot_cue_bank_lists: Vec<HotCueBankList>,
    pub hot_cue_bank_list_entries: Vec<HotCueBankListEntry>,
    pub histories: Vec<HistoryPlaylist>,
    pub history_entries: Vec<HistoryEntry>,
    pub artwork: Vec<Artwork>,
    pub mixer_params: Vec<MixerParam>,
    pub hot_cue_bank_points: Vec<HotCueBankPoint>,
    pub menu_items: Vec<MenuItem>,
    pub categories: Vec<Category>,
    pub sorts: Vec<Sort>,
    pub properties: Vec<Property>,
}

impl ExportDb {
    pub fn read(pdb: &Pdb) -> Result<Self> {
        Ok(Self {
            tracks: read_table(pdb)?,
            genres: read_table(pdb)?,
            artists: read_table(pdb)?,
            albums: read_table(pdb)?,
            labels: read_table(pdb)?,
            keys: read_table(pdb)?,
            colors: read_table(pdb)?,
            playlists: read_table(pdb)?,
            playlist_entries: read_table(pdb)?,
            hot_cue_bank_lists: read_table(pdb)?,
            hot_cue_bank_list_entries: read_table(pdb)?,
            histories: read_table(pdb)?,
            history_entries: read_table(pdb)?,
            artwork: read_table(pdb)?,
            mixer_params: read_table(pdb)?,
            hot_cue_bank_points: read_table(pdb)?,
            menu_items: read_table(pdb)?,
            categories: read_table(pdb)?,
            sorts: read_table(pdb)?,
            properties: read_table(pdb)?,
        })
    }
}

/// Every table of exportExt.pdb, typed.
#[derive(Debug, Clone, Default)]
pub struct ExtDb {
    pub recommend_likes: Vec<RecommendLike>,
    pub histories: Vec<ExtHistory>,
    pub history_entries: Vec<ExtHistoryEntry>,
    pub tags: Vec<MyTag>,
    pub tag_tracks: Vec<MyTagTrack>,
    pub cue_options: Vec<CueOption>,
    pub content_options: Vec<ContentOption>,
    pub db_property_options: Vec<DbPropertyOption>,
    pub general_options: Vec<GeneralOption>,
}

impl ExtDb {
    pub fn read(pdb: &Pdb) -> Result<Self> {
        Ok(Self {
            recommend_likes: read_table(pdb)?,
            histories: read_table(pdb)?,
            history_entries: read_table(pdb)?,
            tags: read_table(pdb)?,
            tag_tracks: read_table(pdb)?,
            cue_options: read_table(pdb)?,
            content_options: read_table(pdb)?,
            db_property_options: read_table(pdb)?,
            general_options: read_table(pdb)?,
        })
    }
}

/// A history playlist with its tracks in play order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceHistory {
    pub id: DbId,
    pub name: String,
    pub tracks: Vec<DbId>,
}

/// A device library as written by rekordbox to `PIONEER/rekordbox/`.
#[derive(Debug, Clone)]
pub struct DeviceLibrary {
    pub export: ExportDb,
    pub ext: Option<ExtDb>,
    /// Directory the files came from, used to resolve device paths (`/Contents/…`).
    pub root: Option<PathBuf>,
}

fn id(v: u32) -> DbId {
    DbId(v.to_string())
}

impl DeviceLibrary {
    /// Opens `<dir>/export.pdb` and, when present, `<dir>/exportExt.pdb`. `dir` is the
    /// `PIONEER/rekordbox` directory; the device root is taken as its grandparent.
    pub fn open(dir: &Path) -> Result<Self> {
        let export = Pdb::open(&dir.join("export.pdb"))?;
        let ext_path = dir.join("exportExt.pdb");
        let ext = if ext_path.exists() {
            Some(Pdb::open(&ext_path)?)
        } else {
            None
        };
        let mut lib = Self::from_pdbs(&export, ext.as_ref())?;
        lib.root = dir.parent().and_then(Path::parent).map(Path::to_owned);
        Ok(lib)
    }

    pub fn from_pdbs(export: &Pdb, ext: Option<&Pdb>) -> Result<Self> {
        Ok(Self {
            export: ExportDb::read(export)?,
            ext: ext.map(ExtDb::read).transpose()?,
            root: None,
        })
    }

    fn device_path(&self, p: &str) -> PathBuf {
        match &self.root {
            Some(root) => root.join(p.trim_start_matches('/')),
            None => PathBuf::from(p),
        }
    }

    /// Converts the device library to the shared domain model: tracks with their lookup names,
    /// the playlist tree and the playlist entries in playlist order. Paths are made absolute
    /// against the device root when the library was opened from disk.
    pub fn snapshot(&self) -> LibrarySnapshot {
        let e = &self.export;
        let artists: HashMap<u32, &str> =
            e.artists.iter().map(|a| (a.id, a.name.as_str())).collect();
        let albums: HashMap<u32, &str> = e.albums.iter().map(|a| (a.id, a.name.as_str())).collect();
        let genres: HashMap<u32, &str> = e
            .genres
            .iter()
            .map(|g| (g.0.id, g.0.name.as_str()))
            .collect();
        let labels: HashMap<u32, &str> = e
            .labels
            .iter()
            .map(|l| (l.0.id, l.0.name.as_str()))
            .collect();
        let keys: HashMap<u32, &str> = e.keys.iter().map(|k| (k.id, k.name.as_str())).collect();
        let artwork: HashMap<u32, &str> =
            e.artwork.iter().map(|a| (a.id, a.path.as_str())).collect();
        let name =
            |m: &HashMap<u32, &str>, k: u32| m.get(&k).map(|s| (*s).to_owned()).unwrap_or_default();
        let nonzero = |v: u32| (v != 0).then_some(v);

        let tracks = e
            .tracks
            .iter()
            .map(|t| mb_core::Track {
                id: id(t.id),
                title: t.title.clone(),
                artist: name(&artists, t.artist_id),
                album: name(&albums, t.album_id),
                genre: name(&genres, t.genre_id),
                label: name(&labels, t.label_id),
                remixer: name(&artists, t.remixer_id),
                composer: name(&artists, t.composer_id),
                original_artist: name(&artists, t.original_artist_id),
                key: name(&keys, t.key_id),
                color: (t.color_id != 0).then_some(t.color_id),
                bpm_x100: nonzero(t.bpm_x100),
                length_seconds: nonzero(u32::from(t.length_s)),
                track_no: nonzero(t.track_no),
                disc_no: nonzero(u32::from(t.disc_no)),
                release_year: nonzero(u32::from(t.release_year)),
                rating: t.rating,
                comment: t.comment.clone(),
                play_count: u32::from(t.play_count),
                bitrate: nonzero(t.bitrate),
                sample_rate: nonzero(t.sample_rate),
                bit_depth: nonzero(u32::from(t.bit_depth)),
                file_size: (t.file_size != 0).then_some(u64::from(t.file_size)),
                file_type: FileType::from_code(i64::from(t.file_type)),
                path: self.device_path(&t.folder_path),
                date_added: t.stock_date.clone(),
                release_date: t.release_date.clone(),
                analysis_path: (!t.analysis_path.is_empty())
                    .then(|| self.device_path(&t.analysis_path)),
                artwork_path: artwork
                    .get(&t.artwork_id)
                    .filter(|p| !p.is_empty())
                    .map(|p| self.device_path(p)),
                analysed: u32::from(t.analysed),
            })
            .collect();

        let playlists = e
            .playlists
            .iter()
            .map(|p| mb_core::PlaylistNode {
                id: id(p.0.id),
                parent: (p.0.parent_id != 0).then(|| id(p.0.parent_id)),
                name: p.0.name.clone(),
                kind: if p.0.attribute == 1 {
                    PlaylistKind::Folder
                } else {
                    PlaylistKind::Playlist
                },
                seq: i64::from(p.0.seq),
            })
            .collect();

        let mut grouped: HashMap<u32, Vec<&PlaylistEntry>> = HashMap::new();
        for entry in &e.playlist_entries {
            grouped.entry(entry.playlist_id).or_default().push(entry);
        }
        let playlist_entries = grouped
            .into_iter()
            .map(|(pid, mut v)| {
                v.sort_by_key(|x| x.entry_index);
                (id(pid), v.into_iter().map(|x| id(x.track_id)).collect())
            })
            .collect();

        let histories = self.history();
        let history_nodes = histories
            .iter()
            .enumerate()
            .map(|(i, h)| mb_core::PlaylistNode {
                id: h.id.clone(),
                parent: None,
                name: h.name.clone(),
                kind: PlaylistKind::Playlist,
                seq: i as i64,
            })
            .collect();
        let history_entries = histories.into_iter().map(|h| (h.id, h.tracks)).collect();

        let (my_tags, my_tag_entries) = match &self.ext {
            Some(ext) => {
                let nodes = ext
                    .tags
                    .iter()
                    .map(|t| mb_core::PlaylistNode {
                        id: id(t.id),
                        parent: (t.parent_id != 0).then(|| id(t.parent_id)),
                        name: t.name.clone(),
                        kind: if t.attribute == 1 { PlaylistKind::Folder } else { PlaylistKind::Playlist },
                        seq: i64::from(t.seq),
                    })
                    .collect();
                let mut entries: HashMap<DbId, Vec<DbId>> = HashMap::new();
                for link in &ext.tag_tracks {
                    entries.entry(id(link.tag_id)).or_default().push(id(link.track_id));
                }
                (nodes, entries)
            }
            None => Default::default(),
        };

        LibrarySnapshot {
            tracks,
            playlists,
            playlist_entries,
            histories: history_nodes,
            history_entries,
            my_tags,
            my_tag_entries,
        }
    }

    /// History playlists (type 11/12) with their tracks in play order, sorted by ID.
    pub fn history(&self) -> Vec<DeviceHistory> {
        let mut out: Vec<DeviceHistory> = self
            .export
            .histories
            .iter()
            .map(|h| {
                let mut entries: Vec<&HistoryEntry> = self
                    .export
                    .history_entries
                    .iter()
                    .filter(|x| x.history_id == h.0.id)
                    .collect();
                entries.sort_by_key(|x| x.seq);
                DeviceHistory {
                    id: id(h.0.id),
                    name: h.0.name.clone(),
                    tracks: entries.iter().map(|x| id(x.track_id)).collect(),
                }
            })
            .collect();
        out.sort_by_key(|h| h.id.0.parse::<u64>().unwrap_or(u64::MAX));
        out
    }
}
