//! Page-by-page byte comparison of two PDB files (writer verification).

use crate::page::HEAP_START;

/// How one page differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageDiff {
    pub page: u32,
    /// Differing bytes in the page header (0x00..0x28; whole page for page 0).
    pub header_bytes: usize,
    /// Differing bytes in the rest of the page (heap and footer, or header-page body).
    pub body_bytes: usize,
    /// First differing offset.
    pub first: usize,
    /// The page exists in only one of the files.
    pub missing: bool,
}

/// Result of [`compare`].
#[derive(Debug, Clone, Default)]
pub struct Comparison {
    pub pages: usize,
    pub identical: usize,
    pub diffs: Vec<PageDiff>,
}

impl Comparison {
    pub fn is_identical(&self) -> bool {
        self.diffs.is_empty()
    }

    pub fn differing_bytes(&self) -> usize {
        self.diffs
            .iter()
            .map(|d| d.header_bytes + d.body_bytes)
            .sum()
    }
}

impl std::fmt::Display for Comparison {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{} pages, {} identical, {} differ ({} bytes)",
            self.pages,
            self.identical,
            self.diffs.len(),
            self.differing_bytes()
        )?;
        for d in &self.diffs {
            if d.missing {
                writeln!(f, "  page {:#x}: only in one file", d.page)?;
            } else {
                writeln!(
                    f,
                    "  page {:#x}: {} header bytes, {} body bytes, first at {:#x}",
                    d.page, d.header_bytes, d.body_bytes, d.first
                )?;
            }
        }
        Ok(())
    }
}

/// Compares two files page by page (4 KiB pages).
pub fn compare(a: &[u8], b: &[u8]) -> Comparison {
    const PS: usize = 4096;
    let pages = a.len().max(b.len()).div_ceil(PS);
    let mut out = Comparison {
        pages,
        ..Default::default()
    };
    for n in 0..pages {
        let pa = a.get(n * PS..(n + 1) * PS);
        let pb = b.get(n * PS..(n + 1) * PS);
        let (Some(pa), Some(pb)) = (pa, pb) else {
            out.diffs.push(PageDiff {
                page: n as u32,
                header_bytes: 0,
                body_bytes: 0,
                first: 0,
                missing: true,
            });
            continue;
        };
        if pa == pb {
            out.identical += 1;
            continue;
        }
        let split = if n == 0 { PS } else { HEAP_START };
        let header_bytes = (0..split).filter(|&i| pa[i] != pb[i]).count();
        let body_bytes = (split..PS).filter(|&i| pa[i] != pb[i]).count();
        let first = (0..PS).find(|&i| pa[i] != pb[i]).unwrap_or(0);
        out.diffs.push(PageDiff {
            page: n as u32,
            header_bytes,
            body_bytes,
            first,
            missing: false,
        });
    }
    out
}
