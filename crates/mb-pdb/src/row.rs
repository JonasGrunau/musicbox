//! Generic, schema-driven row decoding. Spec: `docs/re/pdb.md` § Rows.
//!
//! Header rows (tables with two or more string columns) start with a u32
//! (`TE_RowFormSmgr` @0x102f7572c, `TE_SmgrRowSetOffset` @0x102f76cd0):
//! - bits 2..3: width of the string-offset array entries: 0 = u8, 1 = u16, 2 = u32;
//! - bit 4: deleted flag of the storage layer (always 0 in rekordbox files);
//! - bits 5..20: table number inside the DeviceSQL database (creation order, 1-based);
//! - bits 21..31: slot index of the row in its page.

use crate::page::Slot;
use crate::schema::{ColumnType, RowLayout, TableSchema};
use crate::string::DeviceString;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    U8(u8),
    U16(u16),
    U32(u32),
    Str(DeviceString),
}

impl Value {
    pub fn as_uint(&self) -> Option<u32> {
        match self {
            Self::U8(v) => Some(u32::from(*v)),
            Self::U16(v) => Some(u32::from(*v)),
            Self::U32(v) => Some(*v),
            Self::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&DeviceString> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// The u32 row header of header rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowHeader(pub u32);

impl RowHeader {
    /// Encodes a header (`TE_RowFormSmgr`): `slot << 21 | table_word << 5 | width_code << 2`.
    pub fn new(table_word: u16, offset_width: usize, slot: u16) -> Self {
        let code = match offset_width {
            1 => 0,
            2 => 1,
            _ => 2,
        };
        Self((u32::from(slot & 0x7ff) << 21) | (u32::from(table_word) << 5) | (code << 2))
    }

    /// Bytes per string-offset entry (1, 2 or 4); `None` for the invalid code 3.
    pub fn offset_width(self) -> Option<usize> {
        match (self.0 >> 2) & 3 {
            0 => Some(1),
            1 => Some(2),
            2 => Some(4),
            _ => None,
        }
    }

    pub fn deleted(self) -> bool {
        self.0 & 0x10 != 0
    }

    /// Table number in the DeviceSQL database: type + 1 in export.pdb, type + 49 in exportExt.pdb.
    pub fn table_word(self) -> u16 {
        ((self.0 >> 5) & 0xffff) as u16
    }

    pub fn slot(self) -> u16 {
        (self.0 >> 21) as u16
    }
}

/// One decoded row.
#[derive(Debug, Clone)]
pub struct Row {
    pub schema: &'static TableSchema,
    pub page: u32,
    pub slot: Slot,
    pub header: Option<RowHeader>,
    /// One value per schema column, in declaration order.
    pub values: Vec<Value>,
    /// Row-relative offset of every string column's encoding, by column index.
    pub string_offsets: Vec<Option<usize>>,
    /// Row-relative end of the last byte belonging to the row.
    pub end: usize,
}

impl Row {
    /// Decodes `bytes` (starting at the row) with `schema`.
    pub fn decode(
        schema: &'static TableSchema,
        layout: &RowLayout,
        page: u32,
        slot: Slot,
        bytes: &[u8],
    ) -> Result<Self> {
        let bad = |reason: String| Error::BadRow {
            page,
            slot: slot.index,
            reason,
        };
        let n = schema.columns.len();
        let mut values: Vec<Option<Value>> = vec![None; n];
        let mut string_offsets = vec![None; n];
        let mut end = layout.fixed_end;
        let header = if layout.has_header {
            let raw = bytes
                .get(0..4)
                .ok_or_else(|| bad("row shorter than its header".into()))?;
            Some(RowHeader(u32::from_le_bytes([
                raw[0], raw[1], raw[2], raw[3],
            ])))
        } else {
            None
        };
        for (ci, col) in schema.columns.iter().enumerate() {
            let Some(off) = layout.fixed_offset[ci] else {
                continue;
            };
            let size = col.ty.size().unwrap_or(0);
            let raw = bytes
                .get(off..off + size)
                .ok_or_else(|| bad(format!("column {} beyond row", col.name)))?;
            values[ci] = Some(match col.ty {
                ColumnType::Int8 | ColumnType::Bit8 => Value::U8(raw[0]),
                ColumnType::Int16 | ColumnType::Bit16 => {
                    Value::U16(u16::from_le_bytes([raw[0], raw[1]]))
                }
                _ => Value::U32(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])),
            });
        }
        if let Some(first) = layout.inline_var {
            let s = DeviceString::parse(bytes, layout.var_start)?;
            let mut cursor = layout.var_start + s.encoded_len();
            end = end.max(cursor);
            string_offsets[first] = Some(layout.var_start);
            values[first] = Some(Value::Str(s));
            if !layout.offset_array.is_empty() {
                let width = header
                    .and_then(RowHeader::offset_width)
                    .ok_or_else(|| bad("row header has an invalid offset width".into()))?;
                cursor = cursor.next_multiple_of(width);
                for (k, &ci) in layout.offset_array.iter().enumerate() {
                    let at = cursor + k * width;
                    let raw = bytes
                        .get(at..at + width)
                        .ok_or_else(|| bad("string offset array beyond row".into()))?;
                    let mut buf = [0u8; 4];
                    buf[..width].copy_from_slice(raw);
                    let off = u32::from_le_bytes(buf) as usize;
                    let s = DeviceString::parse(bytes, off)?;
                    end = end.max(off + s.encoded_len());
                    string_offsets[ci] = Some(off);
                    values[ci] = Some(Value::Str(s));
                }
                end = end.max(cursor + width * layout.offset_array.len());
            }
        }
        let values = values
            .into_iter()
            .enumerate()
            .map(|(ci, v)| {
                v.ok_or_else(|| bad(format!("column {} not placed", schema.columns[ci].name)))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            schema,
            page,
            slot,
            header,
            values,
            string_offsets,
            end,
        })
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.schema.column_index(name).map(|i| &self.values[i])
    }

    /// Integer column, widened to u32. Panics on an unknown column: the schemas are static.
    pub fn uint(&self, name: &str) -> u32 {
        self.get(name)
            .and_then(Value::as_uint)
            .unwrap_or_else(|| panic!("{}.{name} is not an integer", self.schema.name))
    }

    /// String column. Panics on an unknown column: the schemas are static.
    pub fn string(&self, name: &str) -> &DeviceString {
        self.get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{}.{name} is not a string", self.schema.name))
    }

    pub fn text(&self, name: &str) -> String {
        self.string(name).text()
    }
}
