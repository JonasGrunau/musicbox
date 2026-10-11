//! Table schemas and the physical row layout. Spec: `docs/re/pdb.md` § Tables, § Rows.
//!
//! The column lists below are the arguments rekordbox 7.2.19 passes to `TETableCreate`
//! @0x102f49484: `_edb_DEV_MODULE1_table_inits` @0x101456e2c and the `__epl_DJDB*_create`
//! functions for export.pdb, and the `__epl_DJDBEX*_create` functions (called from
//! `_edb_MAS_MODULE1_table_inits` @0x100a19e70) for exportExt.pdb. Column order is declaration
//! order; the table type is the creation order among the persistent ("EXTERNAL"/"EXTERNALEX")
//! tables. Generated from the binary; do not edit by hand.

/// DeviceSQL column type names as they appear in the binary (`__epl_*_name`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Int32,
    Bit32,
    Int16,
    Bit16,
    Int8,
    Bit8,
    /// Variable-length string (rekordbox passes UTF-16 and lets the runtime pick the form).
    NVarChar,
    VarChar,
}

impl ColumnType {
    /// Fixed size in bytes, `None` for variable-length columns.
    pub fn size(self) -> Option<usize> {
        match self {
            Self::Int32 | Self::Bit32 => Some(4),
            Self::Int16 | Self::Bit16 => Some(2),
            Self::Int8 | Self::Bit8 => Some(1),
            Self::NVarChar | Self::VarChar => None,
        }
    }

    pub fn is_variable(self) -> bool {
        self.size().is_none()
    }

    /// Size class used by `TE_ComputeRealPositions` @0x102f747b0: 8-byte, 4, 2, 1, variable.
    fn class(self) -> usize {
        match self.size() {
            Some(8) => 0,
            Some(4) => 1,
            Some(2) => 2,
            Some(1) => 3,
            Some(_) => 0,
            None => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub name: &'static str,
    pub ty: ColumnType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableSchema {
    /// Table type in the file's table directory and in every page header.
    pub type_id: u32,
    /// DeviceSQL table name.
    pub name: &'static str,
    pub columns: &'static [Column],
}

/// Which file a schema belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbKind {
    Export,
    ExportExt,
}

impl DbKind {
    pub fn tables(self) -> &'static [TableSchema] {
        match self {
            Self::Export => &EXPORT_TABLES,
            Self::ExportExt => &EXT_TABLES,
        }
    }

    pub fn table(self, type_id: u32) -> Option<&'static TableSchema> {
        self.tables().iter().find(|t| t.type_id == type_id)
    }
}

/// Where each column lives inside a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowLayout {
    /// Rows carry a 4-byte header and a string-offset array. True when the table has two or more
    /// variable-length columns (`TE_DetermineFlatRowType` @0x102f74d98).
    pub has_header: bool,
    /// Column indices in physical order.
    pub physical_order: Vec<usize>,
    /// Byte offset of every fixed-size column, by column index.
    pub fixed_offset: Vec<Option<usize>>,
    /// Column stored inline at `var_start` (the first variable column in physical order).
    pub inline_var: Option<usize>,
    /// 4-aligned offset where the variable part starts.
    pub var_start: usize,
    /// Variable columns reached through the offset array, in array order.
    pub offset_array: Vec<usize>,
    /// Bytes of the fixed part before alignment (header included).
    pub fixed_end: usize,
}

impl TableSchema {
    /// Physical layout, mirroring `TE_ComputeRealPositions` @0x102f747b0 and
    /// `TE_ReComputeOffsets` @0x102f74c3c:
    /// 1. Columns are grouped by size class (8, 4, 2, 1 bytes, variable) in that order.
    /// 2. Inside a class the declaration order is reversed (the last declared column comes first).
    /// 3. Fixed columns are packed without padding from offset 4 (header rows) or 0.
    /// 4. The first variable column sits at the next 4-aligned offset; the remaining variable
    ///    columns are reached through an offset array placed right after it.
    pub fn layout(&self) -> RowLayout {
        let mut count = [0usize; 5];
        for c in self.columns {
            count[c.ty.class()] += 1;
        }
        let mut start = [0usize; 5];
        for i in 1..5 {
            start[i] = start[i - 1] + count[i - 1];
        }
        let mut remaining = count;
        let mut physical_order = vec![0usize; self.columns.len()];
        for (i, c) in self.columns.iter().enumerate() {
            let class = c.ty.class();
            remaining[class] -= 1;
            physical_order[start[class] + remaining[class]] = i;
        }
        let has_header = count[4] >= 2;
        let mut offset = if has_header { 4 } else { 0 };
        let mut fixed_offset = vec![None; self.columns.len()];
        let mut inline_var = None;
        let mut offset_array = Vec::new();
        let fixed_end;
        let mut var_start = 0;
        let first_var = physical_order
            .iter()
            .position(|&ci| self.columns[ci].ty.is_variable());
        match first_var {
            Some(p) => {
                for &ci in &physical_order[..p] {
                    fixed_offset[ci] = Some(offset);
                    offset += self.columns[ci].ty.size().unwrap_or(0);
                }
                fixed_end = offset;
                var_start = offset.next_multiple_of(4);
                inline_var = Some(physical_order[p]);
                offset_array.extend_from_slice(&physical_order[p + 1..]);
            }
            None => {
                for &ci in &physical_order {
                    fixed_offset[ci] = Some(offset);
                    offset += self.columns[ci].ty.size().unwrap_or(0);
                }
                fixed_end = offset;
            }
        }
        RowLayout {
            has_header,
            physical_order,
            fixed_offset,
            inline_var,
            var_start,
            offset_array,
            fixed_end,
        }
    }

    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }
}

pub static EXPORT_TABLES: [TableSchema; 20] = [
    TableSchema {
        type_id: 0,
        name: "DJDBCONTENT",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "FOLDERPATH",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "FILENAMEL",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "FILENAMES",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "TITLE",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "ARTISTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "ALBUMID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "GENREID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "BPM",
                ty: ColumnType::Int32,
            },
            Column {
                name: "LENGTH",
                ty: ColumnType::Int16,
            },
            Column {
                name: "TRACKNO",
                ty: ColumnType::Int32,
            },
            Column {
                name: "BITRATE",
                ty: ColumnType::Int32,
            },
            Column {
                name: "BITDEPTH",
                ty: ColumnType::Int16,
            },
            Column {
                name: "COMMNT",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "FILETYPE",
                ty: ColumnType::Int8,
            },
            Column {
                name: "RATING",
                ty: ColumnType::Int8,
            },
            Column {
                name: "RELEASEYEAR",
                ty: ColumnType::Int16,
            },
            Column {
                name: "REMIXERID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "LABELID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "ORGARTISTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "KEYID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "STOCKDATE",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "COLORID",
                ty: ColumnType::Int8,
            },
            Column {
                name: "DJPLAYCOUNT",
                ty: ColumnType::Int16,
            },
            Column {
                name: "IMAGEID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "MASTERDBID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "MASTERCONTENTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "ANALYSISDATAPATH",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "SEARCHSTR",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "FILESIZE",
                ty: ColumnType::Int32,
            },
            Column {
                name: "DISCNO",
                ty: ColumnType::Int16,
            },
            Column {
                name: "COMPOSERID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "SUBTITLE",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "SAMPLERATE",
                ty: ColumnType::Int32,
            },
            Column {
                name: "DISABLEQUANTIZE",
                ty: ColumnType::Int8,
            },
            Column {
                name: "ANALYSED",
                ty: ColumnType::Int8,
            },
            Column {
                name: "RELEASEDATE",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "DATECREATED",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "CONTENTLINK",
                ty: ColumnType::Int32,
            },
            Column {
                name: "TAG",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED1",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED2",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED3",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED4",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED5",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED6",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED7",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED8",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED9",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED10",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 1,
        name: "DJDBGENRE",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "NAME",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 2,
        name: "DJDBARTIST",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "NAME",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "SEARCHSTR",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 3,
        name: "DJDBALBUM",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "NAME",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "ALBUMARTISTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "IMAGEID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "COMPILATION",
                ty: ColumnType::Int8,
            },
            Column {
                name: "SEARCHSTR",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 4,
        name: "DJDBLABEL",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "NAME",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 5,
        name: "DJDBKEY",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "SCALENAME",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "SEQ",
                ty: ColumnType::Int8,
            },
        ],
    },
    TableSchema {
        type_id: 6,
        name: "DJDBCOLOR",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int8,
            },
            Column {
                name: "COLORCODE",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "SORTKEY",
                ty: ColumnType::Int8,
            },
            Column {
                name: "COMMNT",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 7,
        name: "DJDBPLAYLIST",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "SEQ",
                ty: ColumnType::Int32,
            },
            Column {
                name: "NAME",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "IMAGEID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "ATTRIBUTE",
                ty: ColumnType::Bit8,
            },
            Column {
                name: "PARENTID",
                ty: ColumnType::Int32,
            },
        ],
    },
    TableSchema {
        type_id: 8,
        name: "DJDBSONGPLAYLIST",
        columns: &[
            Column {
                name: "PLAYLISTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "CONTENTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "TRACKNO",
                ty: ColumnType::Int32,
            },
        ],
    },
    TableSchema {
        type_id: 9,
        name: "DJDBHOTCUEBANKLIST",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "SEQ",
                ty: ColumnType::Int32,
            },
            Column {
                name: "NAME",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "IMAGEID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "ATTRIBUTE",
                ty: ColumnType::Bit8,
            },
            Column {
                name: "PARENTID",
                ty: ColumnType::Int32,
            },
        ],
    },
    TableSchema {
        type_id: 10,
        name: "DJDBSONGHOTCUEBANKLIST",
        columns: &[
            Column {
                name: "HOTCUEBANKLISTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "CONTENTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "TRACKNO",
                ty: ColumnType::Int32,
            },
            Column {
                name: "CUEID",
                ty: ColumnType::Int32,
            },
        ],
    },
    TableSchema {
        type_id: 11,
        name: "DJDBHISTORY",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "NAME",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 12,
        name: "DJDBSONGHISTORY",
        columns: &[
            Column {
                name: "HISTORYID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "SEQ",
                ty: ColumnType::Int16,
            },
            Column {
                name: "CONTENTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "ONAIR",
                ty: ColumnType::Int8,
            },
        ],
    },
    TableSchema {
        type_id: 13,
        name: "DJDBIMAGE",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "PATH",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 14,
        name: "DJDBMIXERPARAM",
        columns: &[
            Column {
                name: "CONTENTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "GAIN",
                ty: ColumnType::Int16,
            },
            Column {
                name: "HIGH",
                ty: ColumnType::Int16,
            },
            Column {
                name: "MID",
                ty: ColumnType::Int16,
            },
            Column {
                name: "LOW",
                ty: ColumnType::Int16,
            },
        ],
    },
    TableSchema {
        type_id: 15,
        name: "DJDBHOTCUEBANKPOINT",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "CONTENTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "INMSEC",
                ty: ColumnType::Int32,
            },
            Column {
                name: "INFRAME",
                ty: ColumnType::Int32,
            },
            Column {
                name: "INMPEGFRAME",
                ty: ColumnType::Int32,
            },
            Column {
                name: "INMPEGABS",
                ty: ColumnType::Int32,
            },
            Column {
                name: "OUTMSEC",
                ty: ColumnType::Int32,
            },
            Column {
                name: "OUTFRAME",
                ty: ColumnType::Int32,
            },
            Column {
                name: "OUTMPEGFRAME",
                ty: ColumnType::Int32,
            },
            Column {
                name: "OUTMPEGABS",
                ty: ColumnType::Int32,
            },
            Column {
                name: "KIND",
                ty: ColumnType::Int8,
            },
        ],
    },
    TableSchema {
        type_id: 16,
        name: "DJDBMENUITEMS",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int16,
            },
            Column {
                name: "CLASS",
                ty: ColumnType::Int8,
            },
            Column {
                name: "NAME",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 17,
        name: "DJDBCATEGORY",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int16,
            },
            Column {
                name: "MENUITEMID",
                ty: ColumnType::Int16,
            },
            Column {
                name: "SEQ",
                ty: ColumnType::Int8,
            },
            Column {
                name: "DISABLE",
                ty: ColumnType::Int8,
            },
            Column {
                name: "INFOORDER",
                ty: ColumnType::Int8,
            },
        ],
    },
    TableSchema {
        type_id: 18,
        name: "DJDBSORT",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Int16,
            },
            Column {
                name: "MENUITEMID",
                ty: ColumnType::Int16,
            },
            Column {
                name: "SEQ",
                ty: ColumnType::Int8,
            },
            Column {
                name: "DISABLE",
                ty: ColumnType::Int8,
            },
        ],
    },
    TableSchema {
        type_id: 19,
        name: "DJDBPROPERTY",
        columns: &[
            Column {
                name: "DEVNAME",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "DBVERSION",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "SONGNUMBER",
                ty: ColumnType::Int32,
            },
            Column {
                name: "DATECREATED",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "COLORID",
                ty: ColumnType::Int8,
            },
            Column {
                name: "PROTECTED",
                ty: ColumnType::Int8,
            },
        ],
    },
];

pub static EXT_TABLES: [TableSchema; 9] = [
    TableSchema {
        type_id: 0,
        name: "DJDBEXRECOMMENDLIKE",
        columns: &[
            Column {
                name: "CONTENTID1",
                ty: ColumnType::Int32,
            },
            Column {
                name: "CONTENTID2",
                ty: ColumnType::Int32,
            },
            Column {
                name: "LIKERATE",
                ty: ColumnType::Int8,
            },
            Column {
                name: "RESERVED1",
                ty: ColumnType::Bit8,
            },
            Column {
                name: "RESERVED2",
                ty: ColumnType::Bit16,
            },
            Column {
                name: "DATACREATEDH",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "DATACREATEDL",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED3",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 1,
        name: "DJDBEXHISTORY",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "SEQ",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "NAME",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "ATTRIBUTE",
                ty: ColumnType::Bit8,
            },
            Column {
                name: "RESERVED1",
                ty: ColumnType::Bit8,
            },
            Column {
                name: "RESERVED2",
                ty: ColumnType::Bit16,
            },
            Column {
                name: "PARENTID",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "DATECREATED",
                ty: ColumnType::VarChar,
            },
            Column {
                name: "RESERVED3",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED4",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 2,
        name: "DJDBEXSONGHISTORY",
        columns: &[
            Column {
                name: "HISTORYID",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "CONTENTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "TRACKNO",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED1",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 3,
        name: "DJDBEXMYTAG",
        columns: &[
            Column {
                name: "ID",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "SEQ",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "ATTRIBUTE",
                ty: ColumnType::Bit8,
            },
            Column {
                name: "RESERVED1",
                ty: ColumnType::Bit8,
            },
            Column {
                name: "RESERVED2",
                ty: ColumnType::Bit16,
            },
            Column {
                name: "PARENTID",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED3",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED4",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED5",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "NAME",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVED6",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 4,
        name: "DJDBEXSONGMYTAG",
        columns: &[
            Column {
                name: "MYTAGID",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "CONTENTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "RESERVED1",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED2",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 5,
        name: "DJDBEXCUEOPTION",
        columns: &[
            Column {
                name: "CUEID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "COLOR",
                ty: ColumnType::Bit8,
            },
            Column {
                name: "RESERVED1",
                ty: ColumnType::Bit8,
            },
            Column {
                name: "RESERVED2",
                ty: ColumnType::Bit16,
            },
            Column {
                name: "COMMNT",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "BEATSIZE",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "CUEMICRO",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED5",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED6",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED7",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED8",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "INSEEKINFO",
                ty: ColumnType::VarChar,
            },
            Column {
                name: "OUTSEEKINFO",
                ty: ColumnType::VarChar,
            },
        ],
    },
    TableSchema {
        type_id: 6,
        name: "DJDBEXCONTENTOPTION",
        columns: &[
            Column {
                name: "CONTENTID",
                ty: ColumnType::Int32,
            },
            Column {
                name: "RESERVED1",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED2",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED3",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED4",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED5",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVEDSTRING1",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING2",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING3",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING4",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING5",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 7,
        name: "DJDBEXDBPROPERTYOPTION",
        columns: &[
            Column {
                name: "MYTAGMASTERDBID",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED1",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED2",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED3",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED4",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED5",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "DBVERSION",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING1",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING2",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING3",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING4",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING5",
                ty: ColumnType::NVarChar,
            },
        ],
    },
    TableSchema {
        type_id: 8,
        name: "DJDBEXGENERALOPTION",
        columns: &[
            Column {
                name: "TABLEKIND",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "TABLEID",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED1",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED2",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED3",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVED4",
                ty: ColumnType::Bit32,
            },
            Column {
                name: "RESERVEDSTRING1",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING2",
                ty: ColumnType::NVarChar,
            },
            Column {
                name: "RESERVEDSTRING3",
                ty: ColumnType::NVarChar,
            },
        ],
    },
];
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_layout_matches_fixture_offsets() {
        let l = EXPORT_TABLES[0].layout();
        let t = &EXPORT_TABLES[0];
        let off = |n: &str| l.fixed_offset[t.column_index(n).unwrap()].unwrap();
        assert!(l.has_header);
        assert_eq!(off("CONTENTLINK"), 0x04);
        assert_eq!(off("SAMPLERATE"), 0x08);
        assert_eq!(off("BPM"), 0x38);
        assert_eq!(off("ID"), 0x48);
        assert_eq!(off("DISCNO"), 0x4c);
        assert_eq!(off("LENGTH"), 0x54);
        assert_eq!(off("ANALYSED"), 0x56);
        assert_eq!(off("FILETYPE"), 0x5a);
        assert_eq!(l.fixed_end, 0x5b);
        assert_eq!(l.var_start, 0x5c);
        assert_eq!(t.columns[l.inline_var.unwrap()].name, "RESERVED10");
        assert_eq!(l.offset_array.len(), 21);
        assert_eq!(t.columns[l.offset_array[0]].name, "RESERVED9");
        assert_eq!(
            t.columns[*l.offset_array.last().unwrap()].name,
            "FOLDERPATH"
        );
    }

    #[test]
    fn small_tables_have_no_header() {
        for name in [
            "DJDBGENRE",
            "DJDBKEY",
            "DJDBCOLOR",
            "DJDBPLAYLIST",
            "DJDBSONGPLAYLIST",
            "DJDBCATEGORY",
        ] {
            let t = EXPORT_TABLES.iter().find(|t| t.name == name).unwrap();
            assert!(!t.layout().has_header, "{name}");
        }
        let artist = EXPORT_TABLES[2].layout();
        assert!(artist.has_header);
        assert_eq!(artist.var_start, 8);
    }
}
