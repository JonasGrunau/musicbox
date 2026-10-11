//! Conversion of a [`DeviceLibrary`] into the app-wide [`LibrarySnapshot`].
//! Device paths are relative to the stick root (`/Contents/…`, `/PIONEER/…`).

use crate::model::{DeviceLibrary, DeviceTrack, ListAttribute};
use crate::{DevLibDb, Result};
use mb_core::{DbId, FileType, LibrarySnapshot, PlaylistKind, PlaylistNode, Track};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Joins a device path (leading `/`) onto the mount point of the stick.
pub fn on_stick(stick_root: &Path, device_path: &str) -> PathBuf {
    stick_root.join(device_path.trim_start_matches('/'))
}

fn to_track(t: &DeviceTrack, stick_root: &Path) -> Track {
    Track {
        id: DbId(t.id.to_string()),
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        genre: t.genre.clone(),
        label: t.label.clone(),
        remixer: t.remixer.clone(),
        composer: t.composer.clone(),
        original_artist: t.original_artist.clone(),
        key: t.key.clone(),
        color: t.color,
        bpm_x100: t.bpm_x100,
        length_seconds: t.length_seconds,
        track_no: t.track_no,
        disc_no: t.disc_no,
        release_year: t.release_year,
        rating: t.rating,
        comment: t.comment.clone(),
        play_count: t.play_count,
        bitrate: t.bitrate,
        sample_rate: t.sample_rate,
        bit_depth: t.bit_depth,
        file_size: t.file_size,
        file_type: FileType::from_code(t.file_type),
        path: on_stick(stick_root, &t.path),
        date_added: t.date_added.clone(),
        release_date: t.release_date.clone(),
        analysis_path: Some(t.analysis_path.as_str())
            .filter(|p| !p.is_empty())
            .map(|p| on_stick(stick_root, p)),
        artwork_path: t.artwork_path.as_deref().map(|p| on_stick(stick_root, p)),
        analysed: t.analysed_bits,
    }
}

impl DeviceLibrary {
    /// Collection and playlist tree as a [`LibrarySnapshot`]; IDs become decimal strings.
    pub fn to_snapshot(&self, stick_root: &Path) -> LibrarySnapshot {
        let tracks = self
            .tracks
            .iter()
            .map(|t| to_track(t, stick_root))
            .collect();
        let playlists = self
            .playlists
            .iter()
            .map(|p| PlaylistNode {
                id: DbId(p.id.to_string()),
                parent: p.parent.map(|id| DbId(id.to_string())),
                name: p.name.clone(),
                kind: match p.attribute {
                    ListAttribute::Folder => PlaylistKind::Folder,
                    _ => PlaylistKind::Playlist,
                },
                seq: p.seq,
            })
            .collect();
        let playlist_entries = self
            .playlist_entries
            .iter()
            .map(|(list, items)| {
                (
                    DbId(list.to_string()),
                    items.iter().map(|c| DbId(c.to_string())).collect(),
                )
            })
            .collect();
        let (histories, history_entries) = lists(&self.histories, &self.history_entries);
        let mut tag_links: HashMap<u32, Vec<u32>> = HashMap::new();
        for (tag, content) in &self.my_tag_content {
            tag_links.entry(*tag).or_default().push(*content);
        }
        let (my_tags, my_tag_entries) = lists(&self.my_tags, &tag_links);
        LibrarySnapshot {
            tracks,
            playlists,
            playlist_entries,
            histories,
            history_entries,
            my_tags,
            my_tag_entries,
        }
    }
}

/// Converts a device list tree and its link map to the shared model.
fn lists(nodes: &[crate::DeviceListNode], links: &HashMap<u32, Vec<u32>>) -> (Vec<PlaylistNode>, HashMap<DbId, Vec<DbId>>) {
    let nodes = nodes
        .iter()
        .map(|n| PlaylistNode {
            id: DbId(n.id.to_string()),
            parent: n.parent.map(|id| DbId(id.to_string())),
            name: n.name.clone(),
            kind: match n.attribute {
                ListAttribute::Folder => PlaylistKind::Folder,
                _ => PlaylistKind::Playlist,
            },
            seq: n.seq,
        })
        .collect();
    let links = links
        .iter()
        .map(|(list, items)| (DbId(list.to_string()), items.iter().map(|c| DbId(c.to_string())).collect()))
        .collect();
    (nodes, links)
}

impl DevLibDb {
    /// Loads the collection and playlists; `stick_root` is the mount point (parent of `PIONEER`).
    pub fn load_snapshot(&self, stick_root: &Path) -> Result<LibrarySnapshot> {
        let library = DeviceLibrary {
            tracks: self.tracks()?,
            playlists: self.playlists()?,
            playlist_entries: self.playlist_entries()?,
            ..DeviceLibrary::default()
        };
        Ok(library.to_snapshot(stick_root))
    }
}
