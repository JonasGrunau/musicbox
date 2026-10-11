//! Left-hand tree: per library (local, then each device) the collection, playlists, histories
//! and My Tags. When editing is on, the local playlists get context menus and accept dropped
//! tracks.

use crate::actions::{Action, TrackDrag};
use crate::app::{LibraryRef, List, Source};
use crate::devices::{DeviceState, Devices};
use mb_core::{DbId, LibrarySnapshot, PlaylistKind, PlaylistNode};
use mb_library::{Command, NodeKind};

pub fn show(
    ui: &mut egui::Ui,
    library: &LibrarySnapshot,
    devices: &Devices,
    source: &mut Source,
    editing: bool,
) -> Vec<Action> {
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let mut ctx = Ctx { library_ref: LibraryRef::Local, library, source, editing, actions: &mut actions };
        library_lists(ui, "Collection", &mut ctx);
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
                        let mut ctx = Ctx {
                            library_ref: LibraryRef::Device(i),
                            library: snapshot,
                            source: &mut *source,
                            editing: false,
                            actions: &mut actions,
                        };
                        library_lists(ui, "All Tracks", &mut ctx);
                    }
                });
        }
    });
    actions
}

struct Ctx<'a> {
    library_ref: LibraryRef,
    library: &'a LibrarySnapshot,
    source: &'a mut Source,
    /// Write actions are offered (local library only).
    editing: bool,
    actions: &'a mut Vec<Action>,
}

/// Which list tree of a library a section shows.
#[derive(Clone, Copy, PartialEq)]
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

fn library_lists(ui: &mut egui::Ui, label: &str, ctx: &mut Ctx) {
    let collection = Source { library: ctx.library_ref, list: List::Collection };
    if ui.selectable_label(*ctx.source == collection, label).clicked() {
        *ctx.source = collection;
    }
    for section in [Section::Playlists, Section::Histories, Section::MyTags] {
        let editable = ctx.editing && section == Section::Playlists;
        if section.nodes(ctx.library).is_empty() && !editable {
            continue;
        }
        let header = egui::CollapsingHeader::new(section.title())
            .id_salt((section.title(), ctx.library_ref))
            .default_open(section == Section::Playlists)
            .show(ui, |ui| children(ui, section, None, ctx));
        if editable {
            header.header_response.context_menu(|ui| create_menu(ui, None, ctx.actions));
        }
    }
}

fn create_menu(ui: &mut egui::Ui, parent: Option<&DbId>, actions: &mut Vec<Action>) {
    if ui.button("New Playlist").clicked() {
        actions.push(Action::AskCreate { parent: parent.cloned(), kind: NodeKind::Playlist });
        ui.close();
    }
    if ui.button("New Folder").clicked() {
        actions.push(Action::AskCreate { parent: parent.cloned(), kind: NodeKind::Folder });
        ui.close();
    }
}

fn node_menu(ui: &mut egui::Ui, node: &PlaylistNode, actions: &mut Vec<Action>) {
    if node.kind == PlaylistKind::Folder {
        create_menu(ui, Some(&node.id), actions);
        ui.separator();
    }
    if ui.button("Rename").clicked() {
        actions.push(Action::AskRename { id: node.id.clone(), name: node.name.clone() });
        ui.close();
    }
    if ui.button("Delete").clicked() {
        actions.push(Action::Run(Command::DeletePlaylist { id: node.id.clone() }));
        ui.close();
    }
}

fn sorted_children<'a>(nodes: &'a [PlaylistNode], parent: Option<&DbId>) -> Vec<&'a PlaylistNode> {
    let mut out: Vec<_> = nodes.iter().filter(|n| n.parent.as_ref() == parent).collect();
    out.sort_by_key(|n| n.seq);
    out
}

fn children(ui: &mut egui::Ui, section: Section, parent: Option<&DbId>, ctx: &mut Ctx) {
    let editable = ctx.editing && section == Section::Playlists;
    for node in sorted_children(section.nodes(ctx.library), parent) {
        match node.kind {
            PlaylistKind::Folder => {
                let header = egui::CollapsingHeader::new(&node.name)
                    .id_salt((section.title(), ctx.library_ref, &node.id.0))
                    .show(ui, |ui| children(ui, section, Some(&node.id), ctx));
                if editable {
                    header.header_response.context_menu(|ui| node_menu(ui, node, ctx.actions));
                }
            }
            PlaylistKind::Playlist | PlaylistKind::Smart => {
                let this = Source { library: ctx.library_ref, list: section.list(node.id.clone()) };
                let response = ui.selectable_label(*ctx.source == this, &node.name);
                if response.clicked() {
                    *ctx.source = this;
                }
                if editable && node.kind == PlaylistKind::Playlist {
                    response.context_menu(|ui| node_menu(ui, node, ctx.actions));
                    if response.dnd_hover_payload::<TrackDrag>().is_some() {
                        ui.painter().rect_stroke(
                            response.rect,
                            2.0,
                            egui::Stroke::new(1.5, crate::theme::ACCENT),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if let Some(drag) = response.dnd_release_payload::<TrackDrag>() {
                        ctx.actions.push(Action::Run(Command::AddTracks {
                            playlist: node.id.clone(),
                            content_ids: drag.0.clone(),
                            position: None,
                        }));
                    }
                }
            }
        }
    }
}
