//! The native macOS menu bar (the `muda` crate). Items send [`MenuCommand`]s to the app, which
//! handles them in `App::menu_command`; the headless UI tests call that directly.

use crate::app::MenuCommand;
use muda::{AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use muda::accelerator::{Accelerator, Code, Modifiers};
use std::sync::mpsc::{Receiver, channel};

pub struct MenuBar {
    /// Kept alive: AppKit holds only a weak view of it.
    _menu: Menu,
    events: Receiver<MenuCommand>,
    open_xml: MenuItem,
    close_xml: MenuItem,
    export_xml: MenuItem,
    undo: MenuItem,
    redo: MenuItem,
    /// Last enabled states sent to AppKit (open, close, export, undo/redo).
    enabled: Option<[bool; 4]>,
}

const ITEMS: [(MenuCommand, &str); 6] = [
    (MenuCommand::OpenXml, "open-xml"),
    (MenuCommand::CloseXml, "close-xml"),
    (MenuCommand::ExportXml, "export-xml"),
    (MenuCommand::Undo, "undo"),
    (MenuCommand::Redo, "redo"),
    (MenuCommand::Quit, "quit"),
];

fn item(id: &str, text: &str, accelerator: Option<Accelerator>) -> MenuItem {
    MenuItem::with_id(id, text, true, accelerator)
}

impl MenuBar {
    /// Builds the menu bar and makes it the app's main menu. Main thread only (AppKit).
    pub fn install(ctx: &egui::Context) -> anyhow::Result<Self> {
        let cmd = |code| Some(Accelerator::new(Modifiers::META, code));
        let open_xml = item("open-xml", "Open rekordbox xml…", cmd(Code::KeyO));
        let close_xml = item("close-xml", "Close rekordbox xml", None);
        let export_xml = item("export-xml", "Export Collection in xml format…", cmd(Code::KeyE));
        // Undo and redo carry no key equivalents: ⌘Z stays with egui, so a focused text field
        // undoes its typing and the library undo runs otherwise (`App::global_input`).
        let undo = item("undo", "Undo", None);
        let redo = item("redo", "Redo", None);
        let about = AboutMetadata { name: Some("musicbox".into()), version: Some(env!("CARGO_PKG_VERSION").into()), ..Default::default() };
        let app = Submenu::with_items(
            "musicbox",
            true,
            &[
                &PredefinedMenuItem::about(Some("About musicbox"), Some(about)),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(Some("Hide musicbox")),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                // A close request (not AppKit's terminate), so the app shuts its library
                // connection down normally.
                &item("quit", "Quit musicbox", cmd(Code::KeyQ)),
            ],
        )?;
        let file = Submenu::with_items(
            "File",
            true,
            &[&open_xml, &close_xml, &PredefinedMenuItem::separator(), &export_xml, &PredefinedMenuItem::separator(), &PredefinedMenuItem::close_window(None)],
        )?;
        let edit = Submenu::with_items("Edit", true, &[&undo, &redo])?;
        let window = Submenu::with_items(
            "Window",
            true,
            &[&PredefinedMenuItem::minimize(None), &PredefinedMenuItem::maximize(Some("Zoom")), &PredefinedMenuItem::separator(), &PredefinedMenuItem::fullscreen(None), &PredefinedMenuItem::separator(), &PredefinedMenuItem::bring_all_to_front(None)],
        )?;
        let menu = Menu::with_items(&[&app, &file, &edit, &window])?;
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();

        let (tx, events) = channel();
        let repaint = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((command, _)) = ITEMS.iter().find(|(_, id)| event.id == *id) {
                let _ = tx.send(*command);
                repaint.request_repaint();
            }
        }));
        Ok(Self { _menu: menu, events, open_xml, close_xml, export_xml, undo, redo, enabled: None })
    }

    /// Commands chosen since the last frame.
    pub fn poll(&self) -> Vec<MenuCommand> {
        self.events.try_iter().collect()
    }

    /// Greys out what does not apply right now.
    pub fn sync(&mut self, enabled: [bool; 4]) {
        if self.enabled == Some(enabled) {
            return;
        }
        let [open, close, export, edit] = enabled;
        self.open_xml.set_enabled(open);
        self.close_xml.set_enabled(close);
        self.export_xml.set_enabled(export);
        self.undo.set_enabled(edit);
        self.redo.set_enabled(edit);
        self.enabled = Some(enabled);
    }
}
