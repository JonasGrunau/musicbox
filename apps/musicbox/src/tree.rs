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
    xml: Option<(&std::path::Path, &LibrarySnapshot)>,
    source: &mut Source,
    editing: bool,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let targets: Vec<(usize, String, Option<String>)> =
        devices.list.iter().enumerate().map(|(i, d)| (i, d.name.clone(), d.write_refusal.clone())).collect();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let mut ctx = Ctx { targets: &targets, library_ref: LibraryRef::Local, library, source, editing, device: None, import_from: None, actions: &mut actions };
        library_lists(ui, "Collection", &mut ctx);
        if !devices.list.is_empty() {
            ui.add_space(8.0);
            ui.weak("Devices");
        }
        for (i, device) in devices.list.iter().enumerate() {
            let title = if device.write_refusal.is_some() { format!("{} (protected)", device.name) } else { device.name.clone() };
            let response = egui::CollapsingHeader::new(title)
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
                            targets: &[],
                            library_ref: LibraryRef::Device(i),
                            library: snapshot,
                            source: &mut *source,
                            editing: false,
                            device: device.write_refusal.is_none().then_some(i),
                            import_from: Some(i),
                            actions: &mut actions,
                        };
                        library_lists(ui, "All Tracks", &mut ctx);
                    }
                });
            response.header_response.context_menu(|ui| {
                if ui.button("My Settings…").clicked() {
                    actions.push(Action::AskMySettings { device: i });
                    ui.close();
                }
            });
        }
        // rekordbox's bridge view of a rekordbox.xml ("rekordbox xml").
        if let Some((path, snapshot)) = xml {
            ui.add_space(8.0);
            egui::CollapsingHeader::new("rekordbox xml")
                .id_salt("rekordbox xml")
                .default_open(true)
                .show(ui, |ui| {
                    ui.weak(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                    let mut ctx = Ctx {
                        targets: &[],
                        library_ref: LibraryRef::Xml,
                        library: snapshot,
                        source: &mut *source,
                        editing: false,
                        device: None,
                        import_from: None,
                        actions: &mut actions,
                    };
                    library_lists(ui, "All Tracks", &mut ctx);
                });
        }
    });
    actions
}

/// "Import to Collection" on a rekordbox xml list or folder.
fn xml_menu(ui: &mut egui::Ui, node: &PlaylistNode, actions: &mut Vec<Action>) {
    if ui.button("Import to Collection").clicked() {
        actions.push(Action::ImportFromXml { list: Some(node.id.clone()), tracks: Vec::new() });
        ui.close();
    }
}

struct Ctx<'a> {
    /// Export targets: (index, name, refusal) — offered for local playlists.
    targets: &'a [(usize, String, Option<String>)],
    library_ref: LibraryRef,
    library: &'a LibrarySnapshot,
    source: &'a mut Source,
    /// Write actions are offered (local library only).
    editing: bool,
    /// Index of the device these lists belong to, when musicbox may write to it ("Remove from
    /// device").
    device: Option<usize>,
    /// Index of the device these lists belong to ("Import to Collection" / "Import Histories";
    /// reading only, so protected devices too).
    import_from: Option<usize>,
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
        } else if let Some(device) = ctx.import_from.filter(|_| section == Section::Histories) {
            header.header_response.context_menu(|ui| {
                if ui.button("Import Histories").clicked() {
                    ctx.actions.push(Action::ImportHistories { device });
                    ui.close();
                }
            });
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
    if ui.button("New Intelligent Playlist").clicked() {
        actions.push(Action::AskSmartList { parent: parent.cloned(), id: None });
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

/// Names from the top level down to `node` (how `mb-usb` finds a list in both device DBs).
fn name_path(nodes: &[PlaylistNode], node: &PlaylistNode) -> Vec<String> {
    let mut path = vec![node.name.clone()];
    let mut parent = node.parent.clone();
    for _ in 0..nodes.len() {
        let Some(n) = parent.as_ref().and_then(|p| nodes.iter().find(|n| &n.id == p)) else { break };
        path.push(n.name.clone());
        parent = n.parent.clone();
    }
    path.reverse();
    path
}

/// Device list menu: import (playlists only) and, on writable devices, remove.
fn device_menu(ui: &mut egui::Ui, import_from: usize, writable: Option<usize>, path: Vec<String>, is_list: bool, actions: &mut Vec<Action>) {
    if is_list && ui.button("Import to Collection").clicked() {
        actions.push(Action::ImportFromDevice { device: import_from, playlist: Some(path.clone()), tracks: Vec::new() });
        ui.close();
    }
    if let Some(device) = writable
        && ui.button("Remove from device…").clicked()
    {
        actions.push(Action::AskRemoveFromDevice { device, playlists: vec![path], tracks: Vec::new() });
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
                } else if ctx.library_ref == LibraryRef::Xml {
                    header.header_response.context_menu(|ui| xml_menu(ui, node, ctx.actions));
                } else if let Some(device) = ctx.device.filter(|_| section == Section::Playlists) {
                    let path = name_path(section.nodes(ctx.library), node);
                    let import_from = ctx.import_from.unwrap_or(device);
                    header.header_response.context_menu(|ui| device_menu(ui, import_from, Some(device), path, false, ctx.actions));
                }
            }
            PlaylistKind::Playlist | PlaylistKind::Smart => {
                let this = Source { library: ctx.library_ref, list: section.list(node.id.clone()) };
                let response = ui.selectable_label(*ctx.source == this, &node.name);
                if response.clicked() {
                    *ctx.source = this;
                }
                // Smart lists export their current tracks as a plain device playlist.
                let exportable = ctx.library_ref == LibraryRef::Local && section == Section::Playlists;
                if exportable {
                    let targets = ctx.targets;
                    response.context_menu(|ui| {
                        ui.menu_button("Export to", |ui| {
                            if targets.is_empty() {
                                ui.weak("No USB/SD device connected");
                            }
                            for (index, name, refusal) in targets {
                                let button = ui.add_enabled(refusal.is_none(), egui::Button::new(name));
                                let button = match refusal {
                                    Some(reason) => button.on_disabled_hover_text(reason),
                                    None => button,
                                };
                                if button.clicked() {
                                    ctx.actions.push(Action::Export { playlist: node.id.clone(), device: *index });
                                    ui.close();
                                }
                            }
                        });
                        if editable {
                            ui.separator();
                            if node.kind == PlaylistKind::Smart && ui.button("Edit Intelligent Playlist…").clicked() {
                                ctx.actions.push(Action::AskSmartList { parent: None, id: Some(node.id.clone()) });
                                ui.close();
                            }
                            node_menu(ui, node, ctx.actions);
                        }
                    });
                } else if ctx.library_ref == LibraryRef::Xml {
                    response.context_menu(|ui| xml_menu(ui, node, ctx.actions));
                } else if let Some(import_from) = ctx.import_from.filter(|_| section == Section::Playlists) {
                    let path = name_path(section.nodes(ctx.library), node);
                    let writable = ctx.device;
                    response.context_menu(|ui| device_menu(ui, import_from, writable, path, true, ctx.actions));
                }
                if editable && node.kind == PlaylistKind::Playlist {
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
