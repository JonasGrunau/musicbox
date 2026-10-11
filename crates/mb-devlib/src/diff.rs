//! Table-by-table, row-by-row, column-by-column comparison of two exportLibrary.db files, plus
//! schema text and file-level settings. Used to score the writer against rekordbox's output.
//!
//! Rows are paired by `rowid` (the `<table>_id` key on tables that have one, insertion order on
//! the link tables and `property`). Values are compared with their storage class, so integer 6
//! and text '6', or NULL and 0, count as different.

use rusqlite::Connection;
use rusqlite::types::Value;
use std::fmt;

/// One differing cell.
#[derive(Debug, Clone, PartialEq)]
pub struct CellDiff {
    pub column: String,
    pub left: Value,
    pub right: Value,
}

/// One row present on both sides with at least one differing cell.
#[derive(Debug, Clone, PartialEq)]
pub struct RowDiff {
    pub rowid: i64,
    pub cells: Vec<CellDiff>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableDiff {
    pub table: String,
    pub left_rows: usize,
    pub right_rows: usize,
    pub identical_rows: usize,
    pub differing: Vec<RowDiff>,
    /// rowids only on the left / right side.
    pub only_left: Vec<i64>,
    pub only_right: Vec<i64>,
    /// Column lists differ (then no rows are compared).
    pub columns_differ: bool,
}

impl TableDiff {
    pub fn is_identical(&self) -> bool {
        !self.columns_differ
            && self.differing.is_empty()
            && self.only_left.is_empty()
            && self.only_right.is_empty()
    }
}

/// A file-level setting compared on both sides.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub name: String,
    pub left: String,
    pub right: String,
}

#[derive(Debug, Clone, Default)]
pub struct Comparison {
    /// `sqlite_master` rows (type, name, tbl_name, sql) in rowid order whose text differs,
    /// as (left, right); a missing entry is "".
    pub schema: Vec<(String, String)>,
    /// `sqlite_master.rootpage` differences (page layout only, not content).
    pub rootpages: Vec<(String, i64, i64)>,
    pub settings: Vec<Setting>,
    pub tables: Vec<TableDiff>,
}

impl Comparison {
    /// Schema text, every setting except page layout ones, and every row are equal.
    pub fn content_identical(&self) -> bool {
        self.schema.is_empty()
            && self
                .settings
                .iter()
                .filter(|s| !LAYOUT_SETTINGS.contains(&s.name.as_str()))
                .all(|s| s.left == s.right)
            && self.tables.iter().all(TableDiff::is_identical)
    }
}

/// Settings that describe the page layout rather than the content.
const LAYOUT_SETTINGS: [&str; 2] = ["page_count", "freelist_count"];

const SETTINGS: [&str; 14] = [
    "page_size",
    "journal_mode",
    "user_version",
    "application_id",
    "auto_vacuum",
    "encoding",
    "page_count",
    "freelist_count",
    "cipher_page_size",
    "kdf_iter",
    "cipher_hmac_algorithm",
    "cipher_kdf_algorithm",
    "cipher_use_hmac",
    "cipher_plaintext_header_size",
];

fn text(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => r.to_string(),
        Value::Text(t) => format!("'{t}'"),
        Value::Blob(b) => format!(
            "x'{}'",
            b.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
    }
}

fn pragma(conn: &Connection, name: &str) -> String {
    conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, Value>(0))
        .map(|v| match v {
            Value::Text(t) => t,
            other => text(&other),
        })
        .unwrap_or_else(|e| format!("error: {e}"))
}

fn master_rows(conn: &Connection) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT type || ' ' || name || ' on ' || tbl_name, coalesce(sql, ''), rootpage \
         FROM sqlite_master ORDER BY rowid",
    )?;
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect()
}

fn table_names(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY rowid")?;
    stmt.query_map([], |r| r.get(0))?.collect()
}

type Rows = (Vec<String>, Vec<(i64, Vec<Value>)>);

fn rows(conn: &Connection, table: &str) -> rusqlite::Result<Rows> {
    let mut stmt = conn.prepare(&format!(
        "SELECT rowid, * FROM \"{}\" ORDER BY rowid",
        table.replace('"', "\"\"")
    ))?;
    let columns: Vec<String> = stmt
        .column_names()
        .iter()
        .skip(1)
        .map(|c| c.to_string())
        .collect();
    let n = columns.len();
    let data = stmt
        .query_map([], |r| {
            let rowid: i64 = r.get(0)?;
            let values = (1..=n)
                .map(|i| r.get::<_, Value>(i))
                .collect::<rusqlite::Result<_>>()?;
            Ok((rowid, values))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok((columns, data))
}

fn compare_table(
    left: &Connection,
    right: &Connection,
    table: &str,
) -> rusqlite::Result<TableDiff> {
    let (lc, lr) = rows(left, table)?;
    let (rc, rr) = rows(right, table)?;
    let mut diff = TableDiff {
        table: table.to_string(),
        left_rows: lr.len(),
        right_rows: rr.len(),
        ..TableDiff::default()
    };
    if lc != rc {
        diff.columns_differ = true;
        return Ok(diff);
    }
    let (mut i, mut j) = (0, 0);
    while i < lr.len() || j < rr.len() {
        match (lr.get(i), rr.get(j)) {
            (Some((a, av)), Some((b, bv))) if a == b => {
                let cells: Vec<CellDiff> = lc
                    .iter()
                    .zip(av.iter().zip(bv))
                    .filter(|(_, (x, y))| x != y)
                    .map(|(c, (x, y))| CellDiff {
                        column: c.clone(),
                        left: x.clone(),
                        right: y.clone(),
                    })
                    .collect();
                if cells.is_empty() {
                    diff.identical_rows += 1;
                } else {
                    diff.differing.push(RowDiff { rowid: *a, cells });
                }
                i += 1;
                j += 1;
            }
            (Some((a, _)), Some((b, _))) if a < b => {
                diff.only_left.push(*a);
                i += 1;
            }
            (Some(_), Some((b, _))) => {
                diff.only_right.push(*b);
                j += 1;
            }
            (Some((a, _)), None) => {
                diff.only_left.push(*a);
                i += 1;
            }
            (None, Some((b, _))) => {
                diff.only_right.push(*b);
                j += 1;
            }
            (None, None) => break,
        }
    }
    Ok(diff)
}

/// Compares two open (keyed) databases. Tables are taken from both sides in creation order.
pub fn compare(left: &Connection, right: &Connection) -> rusqlite::Result<Comparison> {
    let mut out = Comparison::default();
    let lm = master_rows(left)?;
    let rm = master_rows(right)?;
    for k in 0..lm.len().max(rm.len()) {
        let l = lm.get(k);
        let r = rm.get(k);
        let lt = l.map(|(h, s, _)| format!("{h}: {s}")).unwrap_or_default();
        let rt = r.map(|(h, s, _)| format!("{h}: {s}")).unwrap_or_default();
        if lt != rt {
            out.schema.push((lt, rt));
        }
        if let (Some((h, _, lp)), Some((_, _, rp))) = (l, r)
            && lp != rp
        {
            out.rootpages.push((h.clone(), *lp, *rp));
        }
    }
    for name in SETTINGS {
        out.settings.push(Setting {
            name: name.to_string(),
            left: pragma(left, name),
            right: pragma(right, name),
        });
    }
    let mut tables = table_names(left)?;
    for t in table_names(right)? {
        if !tables.contains(&t) {
            tables.push(t);
        }
    }
    let right_tables = table_names(right)?;
    let left_tables = table_names(left)?;
    for t in tables {
        if !left_tables.contains(&t) || !right_tables.contains(&t) {
            out.tables.push(TableDiff {
                table: t,
                columns_differ: true,
                ..TableDiff::default()
            });
            continue;
        }
        out.tables.push(compare_table(left, right, &t)?);
    }
    Ok(out)
}

impl fmt::Display for Comparison {
    /// A report: settings, schema, then one line per table and up to 20 differing rows each.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "== settings (left | right)")?;
        for s in &self.settings {
            let mark = if s.left == s.right { " " } else { "≠" };
            writeln!(f, "{mark} {:30} {} | {}", s.name, s.left, s.right)?;
        }
        writeln!(
            f,
            "== schema: {} differing sqlite_master entries",
            self.schema.len()
        )?;
        for (l, r) in &self.schema {
            writeln!(f, "  left : {l}\n  right: {r}")?;
        }
        if !self.rootpages.is_empty() {
            writeln!(f, "== root pages differ (layout only):")?;
            for (h, l, r) in &self.rootpages {
                writeln!(f, "  {h}: {l} | {r}")?;
            }
        }
        writeln!(f, "== tables")?;
        for t in &self.tables {
            if t.columns_differ {
                writeln!(f, "≠ {:20} column lists differ or table missing", t.table)?;
                continue;
            }
            let mark = if t.is_identical() { " " } else { "≠" };
            writeln!(
                f,
                "{mark} {:20} rows {}|{}  identical {}  differing {}  only-left {}  only-right {}",
                t.table,
                t.left_rows,
                t.right_rows,
                t.identical_rows,
                t.differing.len(),
                t.only_left.len(),
                t.only_right.len()
            )?;
            for row in t.differing.iter().take(20) {
                let cells: Vec<String> = row
                    .cells
                    .iter()
                    .map(|c| format!("{}: {} | {}", c.column, text(&c.left), text(&c.right)))
                    .collect();
                writeln!(f, "    rowid {}: {}", row.rowid, cells.join("; "))?;
            }
            if t.differing.len() > 20 {
                writeln!(f, "    … {} more", t.differing.len() - 20)?;
            }
            if !t.only_left.is_empty() {
                writeln!(f, "    only left: {:?}", t.only_left)?;
            }
            if !t.only_right.is_empty() {
                writeln!(f, "    only right: {:?}", t.only_right)?;
            }
        }
        Ok(())
    }
}
