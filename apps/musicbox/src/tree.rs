//! Left-hand tree: per library (local, then each device) the collection, playlists, histories
//! and My Tags.

use crate::app::{LibraryRef, List, Source};
use crate::devices::{DeviceState, Devices};
use mb_core::{DbId, LibrarySnapshot, PlaylistKind, PlaylistNode};

pub fn show(ui: &mut egui::Ui, library: &LibrarySnapshot, devices: &Devices, source: &mut Source) {
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        library_lists(ui, LibraryRef::Local, "Collection", library, source);
        if !devices.list.is_empty() {
            ui.add_space(8.0);
            ui.weak("Devices");
        }
        for (i, device) in devices.list.iter().enumerate() {
            egui::CollapsingHeader::new(&device.name)
                .id_salt(("device", &device.root))
                .default_open(true)
                .show(ui, |ui| match &device.state {
                    DeviceState::Loading(_) => {
                        ui.spinner();
                    }
                    DeviceState::Failed(e) => {
                        ui.weak(e);
                    }
                    DeviceState::Ready(snapshot) => {
                        library_lists(ui, LibraryRef::Device(i), "All Tracks", snapshot, source);
                    }
                });
        }
    });
}

/// Which list tree of a library a section shows.
#[derive(Clone, Copy, Hash)]
enum Section {
    Playlists,
    Histories,
    MyTags,
}

impl Section {
    fn nodes(self, library: &LibrarySnapshot) -> &[PlaylistNode] {
        match self {
            Self::Playlists => &library.playlists,
            Self::Histories => &library.histories,
            Self::MyTags => &library.my_tags,
        }
    }

    fn list(self, id: DbId) -> List {
        match self {
            Self::Playlists => List::Playlist(id),
            Self::Histories => List::History(id),
            Self::MyTags => List::MyTag(id),
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Playlists => "Playlists",
            Self::Histories => "Histories",
            Self::MyTags => "My Tag",
        }
    }
}

fn library_lists(ui: &mut egui::Ui, library_ref: LibraryRef, label: &str, library: &LibrarySnapshot, source: &mut Source) {
    let collection = Source { library: library_ref, list: List::Collection };
    if ui.selectable_label(*source == collection, label).clicked() {
        *source = collection;
    }
    for section in [Section::Playlists, Section::Histories, Section::MyTags] {
        if section.nodes(library).is_empty() {
            continue;
        }
        egui::CollapsingHeader::new(section.title())
            .id_salt((section.title(), library_ref))
            .default_open(matches!(section, Section::Playlists))
            .show(ui, |ui| children(ui, library_ref, library, section, None, source));
    }
}

fn sorted_children<'a>(nodes: &'a [PlaylistNode], parent: Option<&DbId>) -> Vec<&'a PlaylistNode> {
    let mut out: Vec<_> = nodes.iter().filter(|n| n.parent.as_ref() == parent).collect();
    out.sort_by_key(|n| n.seq);
    out
}

fn children(
    ui: &mut egui::Ui,
    library_ref: LibraryRef,
    library: &LibrarySnapshot,
    section: Section,
    parent: Option<&DbId>,
    source: &mut Source,
) {
    for node in sorted_children(section.nodes(library), parent) {
        match node.kind {
            PlaylistKind::Folder => {
                egui::CollapsingHeader::new(&node.name)
                    .id_salt((section.title(), library_ref, &node.id.0))
                    .show(ui, |ui| children(ui, library_ref, library, section, Some(&node.id), source));
            }
            PlaylistKind::Playlist | PlaylistKind::Smart => {
                let this = Source { library: library_ref, list: section.list(node.id.clone()) };
                if ui.selectable_label(*source == this, &node.name).clicked() {
                    *source = this;
                }
            }
        }
    }
}
