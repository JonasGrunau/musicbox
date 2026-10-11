//! Left-hand tree: local Collection + playlists, then each device with its own lists.

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

fn library_lists(ui: &mut egui::Ui, library_ref: LibraryRef, label: &str, library: &LibrarySnapshot, source: &mut Source) {
    let collection = Source { library: library_ref, list: List::Collection };
    if ui.selectable_label(*source == collection, label).clicked() {
        *source = collection;
    }
    egui::CollapsingHeader::new("Playlists")
        .id_salt(("playlists", library_ref))
        .default_open(true)
        .show(ui, |ui| children(ui, library_ref, library, None, source));
}

fn sorted_children<'a>(library: &'a LibrarySnapshot, parent: Option<&DbId>) -> Vec<&'a PlaylistNode> {
    let mut nodes: Vec<_> = library.playlists.iter().filter(|n| n.parent.as_ref() == parent).collect();
    nodes.sort_by_key(|n| n.seq);
    nodes
}

fn children(ui: &mut egui::Ui, library_ref: LibraryRef, library: &LibrarySnapshot, parent: Option<&DbId>, source: &mut Source) {
    for node in sorted_children(library, parent) {
        match node.kind {
            PlaylistKind::Folder => {
                egui::CollapsingHeader::new(&node.name)
                    .id_salt(("folder", library_ref, &node.id.0))
                    .show(ui, |ui| children(ui, library_ref, library, Some(&node.id), source));
            }
            PlaylistKind::Playlist | PlaylistKind::Smart => {
                let this = Source { library: library_ref, list: List::Playlist(node.id.clone()) };
                if ui.selectable_label(*source == this, &node.name).clicked() {
                    *source = this;
                }
            }
        }
    }
}
