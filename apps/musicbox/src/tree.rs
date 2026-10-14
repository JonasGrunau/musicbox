//! Left-hand tree: per library (local, then each device) the collection, playlists, histories
//! and My Tags. When editing is on, the local playlists get context menus and accept dropped
//! tracks.

use crate::actions::{Action, TrackDrag};
use crate::app::{LibraryRef, List, Source};
use crate::devices::{DeviceState, Devices};
use crate::icons::{self, Icon};
use crate::theme;
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
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(6.0);
        let mut ctx = Ctx { targets: &targets, library_ref: LibraryRef::Local, library, source, editing, device: None, import_from: None, actions: &mut actions, depth: 0 };
        library_lists(ui, "Collection", &mut ctx);
        if !devices.list.is_empty() {
            caption_row(ui, "Devices");
        }
        for (i, device) in devices.list.iter().enumerate() {
            let id = egui::Id::new(("device", &device.root));
            let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, true));
            let row = Row { depth: 0, icon: Icon::Device, label: &device.name, selected: false, open: Some(open), count: None };
            let response = row.show(ui);
            if device.write_refusal.is_some() {
                let lock = egui::Rect::from_center_size(egui::pos2(response.rect.right() - 16.0, response.rect.center().y), egui::vec2(12.0, 12.0));
                icons::paint(ui.painter(), lock, Icon::Lock, theme::TEXT_MUTED);
            }
            let response = match &device.write_refusal {
                Some(reason) => response.on_hover_text(reason),
                None => response,
            };
            if response.clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(id, open));
            }
            response.context_menu(|ui| {
                if ui.button("My Settings…").clicked() {
                    actions.push(Action::AskMySettings { device: i });
                    ui.close();
                }
            });
            if !open {
                continue;
            }
            match &device.state {
                DeviceState::Loading(_) => {
                    ui.horizontal(|ui| {
                        ui.add_space(INDENT + 22.0);
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.weak("Reading…");
                    });
                }
                DeviceState::Failed(e) => {
                    ui.horizontal(|ui| {
                        ui.add_space(INDENT + 22.0);
                        ui.label(egui::RichText::new(e).color(theme::WARN).small());
                    });
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
                        depth: 1,
                    };
                    library_lists(ui, "All Tracks", &mut ctx);
                }
            }
        }
        // rekordbox's bridge view of a rekordbox.xml ("rekordbox xml").
        if let Some((path, snapshot)) = xml {
            caption_row(ui, "Bridge");
            let id = egui::Id::new("rekordbox xml");
            let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, true));
            let response = Row { depth: 0, icon: Icon::Xml, label: "rekordbox xml", selected: false, open: Some(open), count: None }.show(ui);
            let response = response.on_hover_text(path.display().to_string());
            if response.clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(id, open));
            }
            if open {
                let mut ctx = Ctx {
                    targets: &[],
                    library_ref: LibraryRef::Xml,
                    library: snapshot,
                    source: &mut *source,
                    editing: false,
                    device: None,
                    import_from: None,
                    actions: &mut actions,
                    depth: 1,
                };
                library_lists(ui, "All Tracks", &mut ctx);
            }
        }
        ui.add_space(8.0);
    });
    actions
}

/// Horizontal step per tree level.
const INDENT: f32 = 14.0;

/// One tree row: full width, a chevron when it opens and closes, an icon, the name and a
/// right-aligned count. Selected rows get the selection fill and an accent edge.
struct Row<'a> {
    depth: usize,
    icon: Icon,
    label: &'a str,
    selected: bool,
    /// `Some(open)` for rows that expand.
    open: Option<bool>,
    count: Option<usize>,
}

impl Row<'_> {
    fn show(self, ui: &mut egui::Ui) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), theme::ROW_HEIGHT), egui::Sense::click());
        let kind = if self.open.is_some() { egui::WidgetType::CollapsingHeader } else { egui::WidgetType::SelectableLabel };
        response.widget_info(|| egui::WidgetInfo::selected(kind, ui.is_enabled(), self.selected, self.label));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let painter = ui.painter();
        if self.selected {
            painter.rect_filled(rect, 0.0, theme::SELECTION);
            painter.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(2.0, rect.height())), 0.0, theme::ACCENT);
        } else if response.hovered() {
            painter.rect_filled(rect, 0.0, theme::ROW_HOVER);
        }
        let x = rect.left() + 10.0 + self.depth as f32 * INDENT;
        let cy = rect.center().y;
        if let Some(open) = self.open {
            let chevron = egui::Rect::from_center_size(egui::pos2(x + 5.0, cy), egui::vec2(10.0, 10.0));
            icons::paint(painter, chevron, if open { Icon::ChevronDown } else { Icon::ChevronRight }, theme::TEXT_MUTED);
        }
        let icon_rect = egui::Rect::from_center_size(egui::pos2(x + 22.0, cy), egui::vec2(14.0, 14.0));
        let strong = self.selected || response.hovered();
        icons::paint(painter, icon_rect, self.icon, if self.selected { theme::ACCENT_HOVER } else { theme::TEXT_DIM });
        let text_left = icon_rect.right() + 8.0;
        let mut right = rect.right() - 10.0;
        if let Some(n) = self.count {
            let galley = painter.layout_no_wrap(n.to_string(), egui::FontId::proportional(11.0), theme::TEXT_MUTED);
            right -= galley.size().x;
            painter.galley(egui::pos2(right, cy - galley.size().y / 2.0), galley, theme::TEXT_MUTED);
            right -= 8.0;
        }
        let color = if strong { theme::TEXT_STRONG } else { theme::TEXT };
        let text = egui::WidgetText::from(egui::RichText::new(self.label).color(color));
        let galley = text.into_galley(ui, Some(egui::TextWrapMode::Truncate), (right - text_left).max(0.0), egui::TextStyle::Body);
        ui.painter().galley(egui::pos2(text_left, cy - galley.size().y / 2.0), galley, color);
        response
    }
}

/// Section caption between libraries ("DEVICES").
fn caption_row(ui: &mut egui::Ui, text: &str) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(theme::caption(text));
    });
    ui.add_space(4.0);
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
    /// Tree level of this library's top rows.
    depth: usize,
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

    fn icon(self) -> Icon {
        match self {
            Self::Playlists => Icon::Playlist,
            Self::Histories => Icon::History,
            Self::MyTags => Icon::Tag,
        }
    }

    /// Track count of one list.
    fn count(self, library: &LibrarySnapshot, id: &DbId) -> Option<usize> {
        let entries = match self {
            Self::Playlists => &library.playlist_entries,
            Self::Histories => &library.history_entries,
            Self::MyTags => &library.my_tag_entries,
        };
        entries.get(id).map(Vec::len)
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
    let row = Row { depth: ctx.depth, icon: Icon::Collection, label, selected: *ctx.source == collection, open: None, count: Some(ctx.library.tracks.len()) };
    if row.show(ui).clicked() {
        *ctx.source = collection;
    }
    for section in [Section::Playlists, Section::Histories, Section::MyTags] {
        let editable = ctx.editing && section == Section::Playlists;
        if section.nodes(ctx.library).is_empty() && !editable {
            continue;
        }
        let id = egui::Id::new((section.title(), ctx.library_ref));
        let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, section == Section::Playlists));
        let header = Row { depth: ctx.depth, icon: section.icon(), label: section.title(), selected: false, open: Some(open), count: None }.show(ui);
        if header.clicked() {
            open = !open;
            ui.data_mut(|d| d.insert_temp(id, open));
        }
        if editable {
            header.context_menu(|ui| create_menu(ui, None, ctx.actions));
        } else if let Some(device) = ctx.import_from.filter(|_| section == Section::Histories) {
            header.context_menu(|ui| {
                if ui.button("Import Histories").clicked() {
                    ctx.actions.push(Action::ImportHistories { device });
                    ui.close();
                }
            });
        }
        if open {
            children(ui, section, None, ctx.depth + 1, ctx);
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

fn children(ui: &mut egui::Ui, section: Section, parent: Option<&DbId>, depth: usize, ctx: &mut Ctx) {
    let editable = ctx.editing && section == Section::Playlists;
    for node in sorted_children(section.nodes(ctx.library), parent) {
        match node.kind {
            PlaylistKind::Folder => {
                let id = egui::Id::new((section.title(), ctx.library_ref, &node.id.0));
                let mut open = ui.data_mut(|d| *d.get_temp_mut_or(id, false));
                let header = Row { depth, icon: Icon::Folder, label: &node.name, selected: false, open: Some(open), count: None }.show(ui);
                if header.clicked() {
                    open = !open;
                    ui.data_mut(|d| d.insert_temp(id, open));
                }
                if editable {
                    header.context_menu(|ui| node_menu(ui, node, ctx.actions));
                } else if ctx.library_ref == LibraryRef::Xml {
                    header.context_menu(|ui| xml_menu(ui, node, ctx.actions));
                } else if let Some(device) = ctx.device.filter(|_| section == Section::Playlists) {
                    let path = name_path(section.nodes(ctx.library), node);
                    let import_from = ctx.import_from.unwrap_or(device);
                    header.context_menu(|ui| device_menu(ui, import_from, Some(device), path, false, ctx.actions));
                }
                if open {
                    children(ui, section, Some(&node.id), depth + 1, ctx);
                }
            }
            PlaylistKind::Playlist | PlaylistKind::Smart => {
                let this = Source { library: ctx.library_ref, list: section.list(node.id.clone()) };
                let icon = match (section, node.kind) {
                    (_, PlaylistKind::Smart) => Icon::Smart,
                    (Section::Playlists, _) => Icon::Playlist,
                    (other, _) => other.icon(),
                };
                let count = section.count(ctx.library, &node.id);
                let response = Row { depth, icon, label: &node.name, selected: *ctx.source == this, open: None, count }.show(ui);
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
