//! Prints the table directory and per-table page/row statistics of a PDB file.
//! Usage: `cargo run -p mb-pdb --example pdb_dump -- <export.pdb|exportExt.pdb> [table_type]`
//! With a table type, also prints every live row of that table.

use mb_pdb::Pdb;
use mb_pdb::schema::DbKind;
use mb_pdb::tables::read_rows;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: pdb_dump <file.pdb> [table_type]")?;
    let only: Option<u32> = args.next().map(|s| s.parse()).transpose()?;
    let pdb = Pdb::open(std::path::Path::new(&path))?;
    let db = if pdb.header.table_count == 9 {
        DbKind::ExportExt
    } else {
        DbKind::Export
    };
    let h = &pdb.header;
    println!(
        "page_size={} tables={} next_unused_page={:#x} state={} xid={}:{} pages_in_file={}",
        h.page_size,
        h.table_count,
        h.next_unused_page,
        h.state,
        h.xid_hi,
        h.xid_lo,
        pdb.page_count()
    );
    for entry in &h.tables {
        let name = db.table(entry.table_type).map_or("?", |t| t.name);
        let pages = pdb.data_pages(entry)?;
        let slots: usize = pages.iter().map(|p| p.header.slot_count as usize).sum();
        let live: usize = pages.iter().map(|p| p.header.live_count as usize).sum();
        println!(
            "type {:2} {:24} first={:#06x} last={:#06x} empty={:#06x} data_pages={:3} slots={:5} live={:5}",
            entry.table_type,
            name,
            entry.first_page,
            entry.last_page,
            entry.empty_candidate,
            pages.len(),
            slots,
            live
        );
        if only == Some(entry.table_type) {
            for row in read_rows(&pdb, db, entry.table_type)? {
                let cols: Vec<String> = row
                    .schema
                    .columns
                    .iter()
                    .zip(&row.values)
                    .map(|(c, v)| match v {
                        mb_pdb::Value::Str(s) => format!("{}={:?}", c.name, s.text()),
                        other => format!("{}={}", c.name, other.as_uint().unwrap_or_default()),
                    })
                    .collect();
                println!("  p{:#x}/s{} {}", row.page, row.slot.index, cols.join(" "));
            }
        }
    }
    Ok(())
}
