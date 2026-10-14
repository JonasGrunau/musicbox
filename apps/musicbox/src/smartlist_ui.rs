//! The intelligent playlist (smart list) rule editor: name, "all/any", one row per condition.
//! The rows map to rekordbox's dialog (`SmartPlaylistComp`, `docs/re/smartlist.md`): property,
//! the operators of its kind, and the value fields that kind uses.

use mb_core::smartlist::{self, Condition, MATCH_ALL, MATCH_ANY, PROPERTIES, SmartList, ValueKind, op};
use mb_core::{DbId, PlaylistKind, PlaylistNode};
use mb_library::Command;

/// rekordbox's colour names, IDs 1..8.
const COLOURS: [&str; 8] = ["Pink", "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple"];

#[derive(Clone, Debug)]
struct Row {
    /// Index into [`PROPERTIES`].
    property: usize,
    operator: i32,
    /// `day` or `month` for "in the last".
    unit: String,
    left: String,
    right: String,
}

impl Row {
    fn new(property: usize) -> Self {
        let kind = PROPERTIES[property].kind;
        Self {
            property,
            operator: kind.operators()[0],
            unit: "day".into(),
            left: default_left(kind),
            right: String::new(),
        }
    }

    fn kind(&self) -> ValueKind {
        PROPERTIES[self.property].kind
    }

    fn from_condition(c: &Condition) -> Option<Self> {
        let property = PROPERTIES.iter().position(|p| p.xml.eq_ignore_ascii_case(&c.property))?;
        let bpm = PROPERTIES[property].xml == "bpm";
        let shown = |v: &str| if bpm { bpm_text(smartlist::juce_int(v)) } else { v.to_owned() };
        Some(Self {
            property,
            operator: c.operator,
            unit: if c.unit.eq_ignore_ascii_case("month") { "month".into() } else { "day".into() },
            left: shown(&c.left),
            right: shown(&c.right),
        })
    }

    /// The condition the dialog stores for this row.
    fn to_condition(&self) -> Condition {
        let p = &PROPERTIES[self.property];
        let in_last = self.operator == op::IN_LAST || self.operator == op::NOT_IN_LAST;
        let unit = if p.kind == ValueKind::Date && in_last { self.unit.as_str() } else { "" };
        let right = if self.operator == op::IN_RANGE { self.right.trim() } else { "" };
        Condition::new(p.xml, self.operator, unit, self.left.trim(), right)
    }
}

fn default_left(kind: ValueKind) -> String {
    match kind {
        ValueKind::Color => "1".into(),
        _ => String::new(),
    }
}

/// BPM×100 as the dialog shows it: `128` or `128.5`.
fn bpm_text(x100: i32) -> String {
    if x100 % 100 == 0 { (x100 / 100).to_string() } else { format!("{:.2}", f64::from(x100) / 100.0).trim_end_matches('0').to_owned() }
}

/// Editor state of the dialog.
pub struct SmartEditor {
    /// The list being edited; `None` for a new one.
    pub id: Option<DbId>,
    pub parent: Option<DbId>,
    original_name: String,
    original_xml: Option<String>,
    pub name: String,
    logical_operator: i32,
    rows: Vec<Row>,
}

impl SmartEditor {
    pub fn new_list(parent: Option<DbId>) -> Self {
        Self {
            id: None,
            parent,
            original_name: String::new(),
            original_xml: None,
            name: "New Intelligent Playlist".into(),
            logical_operator: MATCH_ALL,
            rows: vec![Row::new(1)],
        }
    }

    pub fn edit(node: &PlaylistNode, xml: Option<&str>) -> Self {
        let list = xml.and_then(SmartList::parse);
        let rows: Vec<Row> = list.as_ref().map_or_else(Vec::new, |l| l.conditions.iter().filter_map(Row::from_condition).collect());
        Self {
            id: Some(node.id.clone()),
            parent: node.parent.clone(),
            original_name: node.name.clone(),
            original_xml: xml.map(str::to_owned),
            name: node.name.clone(),
            logical_operator: list.as_ref().map_or(MATCH_ALL, |l| if l.logical_operator == MATCH_ANY { MATCH_ANY } else { MATCH_ALL }),
            rows: if rows.is_empty() { vec![Row::new(1)] } else { rows },
        }
    }

    fn conditions(&self) -> Vec<Condition> {
        self.rows.iter().map(Row::to_condition).collect()
    }

    /// A row the dialog would refuse (`ConditionComp::isValidCondition`): an empty value for an
    /// operator that needs one.
    fn row_valid(r: &Row) -> bool {
        let needs_left = !(r.kind() == ValueKind::Text && (r.operator == op::IS || r.operator == op::IS_NOT));
        (!needs_left || !r.left.trim().is_empty()) && (r.operator != op::IN_RANGE || !r.right.trim().is_empty())
    }

    pub fn valid(&self) -> bool {
        !self.name.trim().is_empty() && !self.rows.is_empty() && self.rows.iter().all(Self::row_valid)
    }

    /// The commands that apply the dialog: create, or the changed rules and name.
    pub fn commands(&self) -> Vec<Command> {
        let name = self.name.trim().to_owned();
        match &self.id {
            None => vec![Command::CreateSmartList {
                parent: self.parent.clone(),
                name,
                logical_operator: self.logical_operator,
                conditions: self.conditions(),
            }],
            Some(id) => {
                let mut out = Vec::new();
                let xml = SmartList::new(id.as_str(), self.logical_operator, self.conditions()).to_xml();
                if self.original_xml.as_deref() != Some(xml.as_str()) {
                    out.push(Command::SetSmartList { id: id.clone(), xml: Some(xml) });
                }
                if name != self.original_name {
                    out.push(Command::RenamePlaylist { id: id.clone(), name });
                }
                out
            }
        }
    }

    pub fn title(&self) -> &'static str {
        if self.id.is_some() { "Edit Intelligent Playlist" } else { "New Intelligent Playlist" }
    }

    /// The dialog body. `my_tags` is the My Tag tree (tags are `Playlist` nodes).
    pub fn show(&mut self, ui: &mut egui::Ui, my_tags: &[PlaylistNode]) {
        ui.horizontal(|ui| {
            ui.label("List Name");
            ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(280.0).margin(egui::vec2(8.0, 4.0)));
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Match");
            egui::ComboBox::from_id_salt("smart-match")
                .selected_text(if self.logical_operator == MATCH_ANY { "any of the" } else { "all of the" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.logical_operator, MATCH_ALL, "all of the");
                    ui.selectable_value(&mut self.logical_operator, MATCH_ANY, "any of the");
                });
            ui.label("following conditions:");
        });
        let tags: Vec<&PlaylistNode> = my_tags.iter().filter(|t| t.kind == PlaylistKind::Playlist).collect();
        let mut remove = None;
        for (i, row) in self.rows.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let before = row.property;
                egui::ComboBox::from_id_salt(("smart-property", i))
                    .selected_text(PROPERTIES[row.property].label)
                    .height(600.0)
                    .show_ui(ui, |ui| {
                        for (p, prop) in PROPERTIES.iter().enumerate() {
                            ui.selectable_value(&mut row.property, p, prop.label);
                        }
                    });
                if row.property != before {
                    *row = Row::new(row.property);
                }
                let kind = row.kind();
                egui::ComboBox::from_id_salt(("smart-operator", i))
                    .selected_text(smartlist::operator_label(row.operator))
                    .show_ui(ui, |ui| {
                        for &o in kind.operators() {
                            ui.selectable_value(&mut row.operator, o, smartlist::operator_label(o));
                        }
                    });
                value_fields(ui, i, row, &tags);
                if ui.small_button("−").on_hover_text("Remove condition").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.rows.remove(i);
        }
        ui.add_space(4.0);
        if ui.button("Add condition").clicked() {
            self.rows.push(Row::new(1));
        }
    }
}

fn value_fields(ui: &mut egui::Ui, i: usize, row: &mut Row, tags: &[&PlaylistNode]) {
    let width = 120.0;
    match row.kind() {
        ValueKind::Text | ValueKind::Number => {
            ui.add(egui::TextEdit::singleline(&mut row.left).desired_width(width).margin(egui::vec2(6.0, 3.0)).id_salt(("smart-left", i)));
        }
        ValueKind::Date => {
            if row.operator == op::IN_LAST || row.operator == op::NOT_IN_LAST {
                ui.add(egui::TextEdit::singleline(&mut row.left).desired_width(48.0).id_salt(("smart-left", i)));
                egui::ComboBox::from_id_salt(("smart-unit", i))
                    .selected_text(if row.unit == "month" { "month(s)" } else { "day(s)" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut row.unit, "day".to_owned(), "day(s)");
                        ui.selectable_value(&mut row.unit, "month".to_owned(), "month(s)");
                    });
            } else {
                ui.add(egui::TextEdit::singleline(&mut row.left).desired_width(width).margin(egui::vec2(6.0, 3.0)).hint_text("YYYY-MM-DD").id_salt(("smart-left", i)));
            }
        }
        ValueKind::MyTag => {
            let current = tags.iter().find(|t| t.id.as_str() == row.left).map_or("(choose)", |t| t.name.as_str());
            egui::ComboBox::from_id_salt(("smart-tag", i)).selected_text(current).show_ui(ui, |ui| {
                for t in tags {
                    ui.selectable_value(&mut row.left, t.id.0.clone(), &t.name);
                }
            });
        }
        ValueKind::Color => {
            let current = row.left.parse::<usize>().ok().and_then(|c| COLOURS.get(c.wrapping_sub(1))).copied().unwrap_or("Pink");
            egui::ComboBox::from_id_salt(("smart-colour", i)).selected_text(current).show_ui(ui, |ui| {
                for (c, name) in COLOURS.iter().enumerate() {
                    ui.selectable_value(&mut row.left, (c + 1).to_string(), *name);
                }
            });
        }
    }
    if row.operator == op::IN_RANGE {
        ui.label("to");
        ui.add(egui::TextEdit::singleline(&mut row.right).desired_width(width).margin(egui::vec2(6.0, 3.0)).id_salt(("smart-right", i)));
    }
}

/// Track IDs of the smart list `id` in `snapshot`, evaluated now in local time.
pub fn evaluate(snapshot: &mb_core::LibrarySnapshot, id: &DbId) -> Option<Vec<DbId>> {
    let xml = snapshot.smart_lists.get(id)?;
    let list = SmartList::parse(xml)?;
    let tags = smartlist::track_tags(snapshot);
    Some(list.evaluate(&snapshot.tracks, &tags, local_clock()).into_iter().cloned().collect())
}

/// Now, with the local UTC offset (`localtime_r`), for "in the last N days/months".
pub fn local_clock() -> smartlist::Clock {
    let clock = smartlist::Clock::utc_now();
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t: libc::time_t = clock.unix as libc::time_t;
    // SAFETY: `localtime_r` writes only into `tm`.
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    smartlist::Clock { unix: clock.unix, utc_offset: if ok { tm.tm_gmtoff } else { 0 } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpm_round_trip_and_commands() {
        assert_eq!(bpm_text(12800), "128");
        assert_eq!(bpm_text(12850), "128.5");
        assert_eq!(bpm_text(12805), "128.05");
        let mut e = SmartEditor::new_list(None);
        e.rows = vec![Row { property: 13, operator: op::IN_RANGE, unit: "day".into(), left: "120".into(), right: "128.5".into() }];
        let Command::CreateSmartList { conditions, .. } = &e.commands()[0] else { panic!() };
        assert_eq!(conditions[0], Condition::new("bpm", op::IN_RANGE, "", "120", "128.5"));
        assert_eq!(conditions[0].right, "12850");
        let back = Row::from_condition(&conditions[0]).unwrap();
        assert_eq!((back.left.as_str(), back.right.as_str()), ("120", "128.5"));
    }
}
