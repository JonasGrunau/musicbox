//! Shared helpers for the oracle tests: temp copies of fixture libraries and a reader for
//! rekordbox's SQL trace (`fixtures/oracle-a/traces/sqltrace-37549.log`). Everything is skipped
//! when the gitignored fixtures are absent.
#![allow(dead_code)]

use mb_masterdb::rusqlite::types::Value;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn fixture(name: &str) -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name);
    root.join("rekordbox/master.db").exists().then_some(root)
}

/// A temp copy of a fixture library: `master.db` (WAL is empty in all fixtures) and
/// `masterPlaylists6.xml`. Removed on drop.
pub struct TempLibrary {
    pub dir: PathBuf,
    pub db: PathBuf,
    pub passphrase: String,
    pub paths: LibraryPaths,
}

impl TempLibrary {
    pub fn new(fixture_name: &str) -> Option<Self> {
        let root = fixture(fixture_name)?;
        let mut paths = LibraryPaths::from_options_json(&root.join("options.json")).unwrap();
        let passphrase = derive_passphrase(&paths.dp).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "mb-library-{fixture_name}-{}-{}",
            std::process::id(),
            mb_masterdb::write::new_rid32().unwrap()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("master.db");
        std::fs::copy(root.join("rekordbox/master.db"), &db).unwrap();
        let xml = root.join("rekordbox/masterPlaylists6.xml");
        if xml.exists() {
            std::fs::copy(&xml, dir.join("masterPlaylists6.xml")).unwrap();
        }
        paths.db_path = db.clone();
        paths.analysis_data_root = root.join("rekordbox/share");
        Some(Self { dir, db, passphrase, paths })
    }

    pub fn xml_path(&self) -> PathBuf {
        self.dir.join("masterPlaylists6.xml")
    }

    /// Re-opens read-only, loads a snapshot and runs `PRAGMA integrity_check`.
    pub fn verify_loads(&self) -> mb_core::LibrarySnapshot {
        let db = MasterDb::open_read_only(&self.db, &self.passphrase).unwrap();
        let check: String = db.connection().query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(check, "ok", "integrity_check");
        db.load_snapshot(&self.paths).unwrap()
    }
}

impl Drop for TempLibrary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn open_read_only(fixture_name: &str) -> Option<MasterDb> {
    let root = fixture(fixture_name)?;
    let paths = LibraryPaths::from_options_json(&root.join("options.json")).unwrap();
    let passphrase = derive_passphrase(&paths.dp).unwrap();
    Some(MasterDb::open_read_only(&root.join("rekordbox/master.db"), &passphrase).unwrap())
}

/// One traced statement.
#[derive(Debug, Clone)]
pub struct TraceLine {
    pub line: usize,
    pub time_ms: i64,
    pub thread: String,
    pub db: String,
    pub sql: String,
}

pub fn trace() -> Option<Vec<TraceLine>> {
    let root = fixture("oracle-a")?;
    let text = std::fs::read(root.join("traces/sqltrace-37549.log")).ok()?;
    let text = String::from_utf8_lossy(&text);
    Some(
        text.lines()
            .enumerate()
            .filter_map(|(i, l)| {
                let mut parts = l.splitn(4, '\t');
                let time_ms = parts.next()?.parse().ok()?;
                let thread = parts.next()?.to_owned();
                let db = parts.next()?.to_owned();
                let sql = parts.next()?.to_owned();
                Some(TraceLine { line: i + 1, time_ms, thread, db, sql })
            })
            .collect(),
    )
}

/// A value literal of rekordbox's builder dialect: `"text"` or a bare number.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Text(String),
    Int(i64),
    Real(f64),
}

impl Lit {
    pub fn as_text(&self) -> String {
        match self {
            Lit::Text(s) => s.clone(),
            Lit::Int(i) => i.to_string(),
            Lit::Real(r) => r.to_string(),
        }
    }
}

fn parse_literals(s: &str) -> Vec<Lit> {
    let b: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            ' ' | ',' => i += 1,
            q @ ('"' | '\'') => {
                let mut buf = String::new();
                let mut j = i + 1;
                loop {
                    if b[j] == q {
                        if j + 1 < b.len() && b[j + 1] == q {
                            buf.push(q);
                            j += 2;
                            continue;
                        }
                        break;
                    }
                    buf.push(b[j]);
                    j += 1;
                }
                out.push(Lit::Text(buf));
                i = j + 1;
            }
            _ => {
                let mut j = i;
                while j < b.len() && b[j] != ',' {
                    j += 1;
                }
                let tok: String = b[i..j].iter().collect::<String>().trim().to_owned();
                out.push(match (tok.parse::<i64>(), tok.parse::<f64>()) {
                    (Ok(i), _) => Lit::Int(i),
                    (_, Ok(r)) => Lit::Real(r),
                    _ => Lit::Text(tok),
                });
                i = j;
            }
        }
    }
    out
}

/// `insert into <T> (<cols>) values (<lits>)` (either dialect) → (table, column → literal).
pub fn parse_insert(sql: &str) -> Option<(String, Vec<(String, Lit)>)> {
    let lower = sql.to_ascii_lowercase();
    if !lower.starts_with("insert into ") {
        return None;
    }
    let open = sql.find('(')?;
    let table = sql["insert into ".len()..open].trim().to_owned();
    let close = sql[open..].find(')')? + open;
    let cols: Vec<String> = sql[open + 1..close].split(',').map(|c| c.trim().to_owned()).collect();
    let vstart = lower[close..].find("values (")? + close + "values (".len();
    let vals = parse_literals(&sql[vstart..sql.len() - 1]);
    if cols.len() != vals.len() {
        return None; // multi-row inserts (device DB) are not needed here
    }
    Some((table, cols.into_iter().zip(vals).collect()))
}

/// One traced import: the `djmdContent` insert and the lookup statements of its transaction.
#[derive(Debug, Clone)]
pub struct TracedImport {
    pub line: usize,
    pub row: HashMap<String, Lit>,
    pub columns: Vec<String>,
    /// (table, statement) of every name lookup in the file's transaction, in order.
    pub lookups: Vec<(String, String)>,
    /// Inserts into lookup tables in the file's transaction, in order: (table, column → literal).
    pub created: Vec<(String, HashMap<String, Lit>)>,
}

impl TracedImport {
    pub fn path(&self) -> PathBuf {
        PathBuf::from(self.row["FolderPath"].as_text())
    }
    pub fn text(&self, col: &str) -> Option<String> {
        self.row.get(col).map(Lit::as_text)
    }
}

/// The 161 imports of the session (trace lines 600–3700), in trace order.
pub fn traced_imports(trace: &[TraceLine]) -> Vec<TracedImport> {
    let mut out = Vec::new();
    let mut lookups = Vec::new();
    let mut created = Vec::new();
    for l in trace.iter().filter(|l| (600..3700).contains(&l.line) && l.db.ends_with("master.db")) {
        if l.sql == "begin exclusive" {
            lookups.clear();
            created.clear();
        } else if l.sql.starts_with("select ID from djmd") && (l.sql.contains(" Name = ") || l.sql.contains(" ScaleName = ")) {
            let table = l.sql["select ID from ".len()..].split(' ').next().unwrap().to_owned();
            lookups.push((table, l.sql.clone()));
        } else if let Some((table, cols)) = parse_insert(&l.sql) {
            if table == "djmdContent" {
                out.push(TracedImport {
                    line: l.line,
                    columns: cols.iter().map(|(c, _)| c.clone()).collect(),
                    row: cols.into_iter().collect(),
                    lookups: std::mem::take(&mut lookups),
                    created: std::mem::take(&mut created),
                });
            } else {
                created.push((table, cols.into_iter().collect()));
            }
        }
    }
    out
}

/// The quoted name in a traced lookup (`… Name = "<x>" …`).
pub fn lookup_name(sql: &str) -> String {
    let key = if sql.contains("ScaleName = \"") { "ScaleName = \"" } else { "Name = \"" };
    let start = sql.find(key).unwrap() + key.len();
    let rest = &sql[start..];
    let mut out = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                out.push('"');
                chars.next();
                continue;
            }
            break;
        }
        out.push(c);
    }
    out
}

/// The tag names a traced import looked up: (artist, genre, album artist, album, remixer, label, key).
pub fn traced_names(imp: &TracedImport) -> [String; 7] {
    let mut names: [String; 7] = Default::default();
    let mut artist_lookups: Vec<String> = Vec::new();
    for (i, (table, sql)) in imp.lookups.iter().enumerate() {
        let name = lookup_name(sql);
        match table.as_str() {
            "djmdArtist" => {
                let next_is_album_with_artist = imp.lookups.get(i + 1).is_some_and(|(t, s)| t == "djmdAlbum" && s.contains("AlbumArtistID ="));
                if next_is_album_with_artist {
                    names[2] = name;
                } else {
                    artist_lookups.push(name);
                }
            }
            "djmdGenre" => names[1] = name,
            "djmdAlbum" => names[3] = name,
            "djmdLabel" => names[5] = name,
            "djmdKey" => names[6] = name,
            _ => {}
        }
    }
    if imp.row.contains_key("ArtistID") && !artist_lookups.is_empty() {
        names[0] = artist_lookups.remove(0);
    }
    if imp.row.contains_key("RemixerID") {
        names[4] = artist_lookups.pop().unwrap_or_default();
    }
    names
}

/// A row as (column → value) for comparisons.
pub fn row(conn: &mb_masterdb::rusqlite::Connection, table: &str, key_col: &str, key: &str) -> Option<HashMap<String, Value>> {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {table} WHERE {key_col} = ?1")).unwrap();
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query([key]).unwrap();
    let r = rows.next().unwrap()?;
    Some(names.iter().enumerate().map(|(i, n)| (n.clone(), r.get::<_, Value>(i).unwrap())).collect())
}

pub fn counter(conn: &mb_masterdb::rusqlite::Connection) -> i64 {
    conn.query_row("SELECT int_1 FROM agentRegistry WHERE registry_id = 'localUpdateCount'", [], |r| r.get(0)).unwrap()
}

pub fn count(conn: &mb_masterdb::rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}
