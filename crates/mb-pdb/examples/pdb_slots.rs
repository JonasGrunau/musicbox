//! Every slot of every data page of one table, deleted ones included, with its offset, present
//! (P) and dirty (D) bits and decoded values (empty strings left out). Useful to read a table's
//! write history (`docs/re/pdb.md` § Storage engine).
//! Usage: `cargo run -p mb-pdb --example pdb_slots -- <file.pdb> <table_type>`
use mb_pdb::Pdb;
use mb_pdb::schema::DbKind;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: pdb_slots <file.pdb> <table_type>")?;
    let t: u32 = args
        .next()
        .ok_or("usage: pdb_slots <file.pdb> <table_type>")?
        .parse()?;
    let pdb = Pdb::open(std::path::Path::new(&path))?;
    let db = if pdb.header.table_count == 9 {
        DbKind::ExportExt
    } else {
        DbKind::Export
    };
    let entry = *pdb.header.table(t).ok_or("no such table")?;
    let schema = db.table(t).ok_or("no such table")?;
    let layout = schema.layout();
    for page in pdb.data_pages(&entry)? {
        println!(
            "page {:#x} xid={} slots={} live={} used={} free={}",
            page.number,
            page.header.xid_lo,
            page.header.slot_count,
            page.header.live_count,
            page.header.used_size,
            page.header.free_size
        );
        for slot in page.slots()? {
            let row =
                mb_pdb::Row::decode(schema, &layout, page.number, slot, page.row_bytes(&slot)?)?;
            let cols: Vec<String> = row
                .schema
                .columns
                .iter()
                .zip(&row.values)
                .filter_map(|(c, v)| match v {
                    mb_pdb::Value::Str(s) => {
                        if s.is_empty() {
                            None
                        } else {
                            Some(format!("{}={:?}", c.name, s.text()))
                        }
                    }
                    other => Some(format!(
                        "{}={}",
                        c.name,
                        other.as_uint().unwrap_or_default()
                    )),
                })
                .collect();
            println!(
                "  s{:3} off={:4} {}{} end={} {}",
                slot.index,
                slot.offset,
                if slot.present { "P" } else { "-" },
                if slot.dirty { "D" } else { " " },
                row.end,
                cols.join(" ")
            );
        }
    }
    Ok(())
}
