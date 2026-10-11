//! Row encoder, the inverse of [`crate::row::Row::decode`]. Spec: `docs/re/pdb.md` § Rows,
//! § Offset width and allocation size, § Writer.
//!
//! Mirrors `TE_RowFormSmgr` @0x102f7572c:
//! - The allocation request is computed first (`size`, below). The storage manager reserves
//!   `align4(size)` bytes for the row.
//! - The row is then written field by field straight into the page. Only the row header, the
//!   fixed columns, the offset-array entries and the string bytes are stored. Alignment gaps are
//!   skipped, so on a reused slot they keep the bytes of the row that lived there before
//!   ([`EncodedRow::written`] lists the stored ranges).
//! - The slot field of the row header (bits 21..31) is set afterwards (`TE_SmgrRowSetOffset`
//!   @0x102f76cd0); [`EncodedRow::set_slot`] does that.

use std::ops::Range;

use crate::row::{RowHeader, Value};
use crate::schema::{ColumnType, DbKind, RowLayout, TableSchema};
use crate::string::DeviceString;
use crate::{Error, Result};

/// Table number stored in header rows: creation order in the DeviceSQL database, 1-based
/// (type + 1 in export.pdb, type + 49 in exportExt.pdb; `docs/re/pdb.md` § Row header).
pub fn table_word(db: DbKind, table_type: u32) -> u16 {
    let base = match db {
        DbKind::Export => 1,
        DbKind::ExportExt => 49,
    };
    (table_type + base) as u16
}

/// One encoded row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedRow {
    /// Row bytes from the row start to the last written byte. Alignment gaps are zero here.
    pub bytes: Vec<u8>,
    /// The byte ranges `TE_RowFormSmgr` stores; everything else in the allocation is left as it was.
    pub written: Vec<Range<usize>>,
    /// Allocation request; the page reserves `align4(size)` bytes.
    pub size: usize,
    /// Header rows only: the row header word without the slot.
    pub header: Option<RowHeader>,
}

impl EncodedRow {
    /// Bytes the row takes in the page heap.
    pub fn alloc(&self) -> usize {
        self.size.next_multiple_of(4)
    }

    /// Stores the slot index in the row header (header rows only).
    pub fn set_slot(&mut self, slot: u16) {
        if let Some(h) = self.header {
            let raw = (h.0 & 0x001f_ffff) | (u32::from(slot & 0x7ff) << 21);
            self.bytes[0..4].copy_from_slice(&raw.to_le_bytes());
        }
    }

    /// Writes the stored ranges into `area` (the row's place in the page), leaving the gaps.
    pub fn write_into(&self, area: &mut [u8]) {
        for r in &self.written {
            area[r.clone()].copy_from_slice(&self.bytes[r.clone()]);
        }
    }
}

/// Width of the string-offset array entries and the allocation request (`TE_RowFormSmgr`).
/// `strings` are the encoded lengths of the variable columns in physical order, inline first.
pub fn alloc_size(layout: &RowLayout, strings: &[usize]) -> (usize, usize) {
    let mut size = layout.fixed_end;
    for &len in strings {
        size = size.next_multiple_of(4) + len;
    }
    let nvar = layout.offset_array.len();
    let width = if nvar == 0 {
        0
    } else if size + nvar.next_multiple_of(4) + 4 < 0x100 {
        1
    } else if size + (2 * nvar).next_multiple_of(4) + 4 < 0x1_0000 {
        2
    } else {
        4
    };
    (width, size + (width * nvar).next_multiple_of(4))
}

fn int_value(v: &Value) -> Option<u32> {
    v.as_uint()
}

/// Encodes one row. `values` holds one value per schema column in declaration order: integers
/// (any [`Value`] integer variant, truncated to the column width) for fixed columns and
/// [`Value::Str`] for string columns.
pub fn encode_row(
    db: DbKind,
    schema: &TableSchema,
    layout: &RowLayout,
    values: &[Value],
) -> Result<EncodedRow> {
    let bad = |reason: String| Error::BadValue {
        table: schema.name,
        reason,
    };
    if values.len() != schema.columns.len() {
        return Err(bad(format!(
            "{} values for {} columns",
            values.len(),
            schema.columns.len()
        )));
    }
    let string_of = |ci: usize| -> Result<&DeviceString> {
        match &values[ci] {
            Value::Str(s) => Ok(s),
            _ => Err(bad(format!("{} needs a string", schema.columns[ci].name))),
        }
    };

    // Variable columns in physical order, inline first.
    let mut var_cols = Vec::new();
    if let Some(ci) = layout.inline_var {
        var_cols.push(ci);
    }
    var_cols.extend_from_slice(&layout.offset_array);
    let strings: Vec<&DeviceString> = var_cols
        .iter()
        .map(|&ci| string_of(ci))
        .collect::<Result<_>>()?;
    let lens: Vec<usize> = strings.iter().map(|s| s.encoded_len()).collect();
    let (width, size) = alloc_size(layout, &lens);

    let mut bytes = Vec::with_capacity(size);
    let mut written = Vec::new();
    let mut put = |bytes: &mut Vec<u8>, at: usize, data: &[u8]| {
        if bytes.len() < at + data.len() {
            bytes.resize(at + data.len(), 0);
        }
        bytes[at..at + data.len()].copy_from_slice(data);
        written.push(at..at + data.len());
    };

    let header = if layout.has_header {
        let h = RowHeader::new(table_word(db, schema.type_id), width.max(1), 0);
        put(&mut bytes, 0, &h.0.to_le_bytes());
        Some(h)
    } else {
        None
    };

    for (ci, col) in schema.columns.iter().enumerate() {
        let Some(off) = layout.fixed_offset[ci] else {
            continue;
        };
        let v =
            int_value(&values[ci]).ok_or_else(|| bad(format!("{} needs an integer", col.name)))?;
        match col.ty {
            ColumnType::Int8 | ColumnType::Bit8 => put(&mut bytes, off, &[v as u8]),
            ColumnType::Int16 | ColumnType::Bit16 => {
                put(&mut bytes, off, &(v as u16).to_le_bytes())
            }
            _ => put(&mut bytes, off, &v.to_le_bytes()),
        }
    }

    if !var_cols.is_empty() {
        let mut cursor = layout.var_start;
        let inline = strings[0];
        put(&mut bytes, cursor, &inline.to_bytes());
        cursor += inline.encoded_len();
        if width > 0 {
            cursor = cursor.next_multiple_of(width);
            let array_at = cursor;
            cursor += width * layout.offset_array.len();
            for (k, s) in strings[1..].iter().enumerate() {
                if !s.is_short() {
                    cursor = cursor.next_multiple_of(4);
                }
                let entry = (cursor as u32).to_le_bytes();
                put(&mut bytes, array_at + k * width, &entry[..width]);
                put(&mut bytes, cursor, &s.to_bytes());
                cursor += s.encoded_len();
            }
        }
    } else if bytes.len() < layout.fixed_end {
        bytes.resize(layout.fixed_end, 0);
    }
    written.sort_by_key(|r| r.start);
    Ok(EncodedRow {
        bytes,
        written,
        size,
        header,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::EXPORT_TABLES;

    #[test]
    fn genre_row() {
        let t = &EXPORT_TABLES[1];
        let l = t.layout();
        let row = encode_row(
            DbKind::Export,
            t,
            &l,
            &[Value::U32(3), Value::Str(DeviceString::from_text("Trance"))],
        )
        .unwrap();
        assert_eq!(row.bytes, b"\x03\0\0\0\x0fTrance".to_vec());
        assert_eq!(row.size, 11);
        assert_eq!(row.alloc(), 12);
    }

    #[test]
    fn category_row_has_no_strings() {
        let t = &EXPORT_TABLES[17];
        let l = t.layout();
        let row = encode_row(
            DbKind::Export,
            t,
            &l,
            &[
                Value::U16(1),
                Value::U16(1),
                Value::U8(0),
                Value::U8(1),
                Value::U8(99),
            ],
        )
        .unwrap();
        assert_eq!(row.bytes, vec![1, 0, 1, 0, 99, 1, 0]);
        assert_eq!(row.alloc(), 8);
    }
}
