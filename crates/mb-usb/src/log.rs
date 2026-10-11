//! The export log rekordbox writes when tracks were not exported.
//! Spec: `docs/re/export.md` § 1.8 (`DbBackGroundProcess::write_error_report` @0x1001d4d7c,
//! `TrackAdditionResult::getErrorInfo` @0x1001d2c1c, `exportProcessChange` @0x1009bf758).
//!
//! File: `~/Documents/rekordbox/Export Log rekordbox <YYYY-MM-DD>.txt`, UTF-16LE with a BOM and
//! CRLF line ends. A header is written when the file is created; every job with a failed track
//! then appends one line per failed track.

use crate::plan::{Job, TrackResult};
use std::path::{Path, PathBuf};

/// The log's fixed texts. rekordbox passes each through `juce::translate`; the defaults are the
/// untranslated (English) strings from the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogStrings {
    pub not_exported: String,
    pub reasons_intro: String,
    /// Reasons `[1]`..`[5]`.
    pub reasons: [String; 5],
    pub track_title: String,
    pub location: String,
}

impl Default for LogStrings {
    fn default() -> Self {
        Self {
            not_exported: "One or more files were not exported. ".into(),
            reasons_intro: "Track(s) were not exported due to the following possible reasons."
                .into(),
            reasons: [
                "The file doesn't exist.".into(),
                "The file is in the format that CDJ/XDJ/mobile device doesn't support.".into(),
                "The file could not be copied.".into(),
                "Track analysis data is corrupted or doesn't exist.".into(),
                "Error occurred in the USB/SD/mobile device.".into(),
            ],
            track_title: "Track Title".into(),
            location: "Location".into(),
        }
    }
}

impl LogStrings {
    /// The column titles of rekordbox's German UI (the only translated strings in a German
    /// log, as in `fixtures/oracle-a/export-log.txt`).
    pub fn german() -> Self {
        Self {
            track_title: "Trackname".into(),
            location: "Lage".into(),
            ..Self::default()
        }
    }
}

/// `"Export Log " + appName + " " + getDateString(now) + ".txt"`.
pub fn log_file_name(today: &str) -> String {
    format!("Export Log rekordbox {today}.txt")
}

/// The header block (`\n` line ends; converted on write).
pub fn header(s: &LogStrings) -> String {
    let mut out = format!("{}\n{}\n", s.not_exported, s.reasons_intro);
    for (i, r) in s.reasons.iter().enumerate() {
        out.push_str(&format!("[{}] - {r}\n", i + 1));
    }
    out.push('\n');
    out.push_str(&format!("    {}\t{}\n", s.track_title, s.location));
    out
}

/// `[n] ` for a result code (`getErrorInfo`): 4 → 1, 5 → 2, 8 → 3, 9 → 4, 7/11/12 → 5,
/// anything else → `[?]` (no space). Successes print nothing.
fn code_prefix(code: u32) -> Option<&'static str> {
    match code {
        0 | 1 | 15 => None,
        4 => Some("[1] "),
        5 => Some("[2] "),
        8 => Some("[3] "),
        9 => Some("[4] "),
        7 | 11 | 12 => Some("[5] "),
        _ => Some("[?]"),
    }
}

fn exists_flag(p: &Path) -> &'static str {
    if p.exists() { "*" } else { "-" }
}

/// One line of a failed track. For analysis failures (code 9) rekordbox appends the analysis
/// path and three flags for whether the `.DAT`, `.EXT` and `.2EX` exist (`*`) or not (`-`).
pub fn entry(result: TrackResult, title: &str, location: &str, analysis: Option<&Path>) -> String {
    let Some(prefix) = code_prefix(result.code()) else {
        return String::new();
    };
    let mut out = format!("{prefix}{title}\t{location}");
    if result == TrackResult::AnalysisFailed {
        out.push('\t');
        if let Some(dat) = analysis.filter(|p| !p.as_os_str().is_empty()) {
            out.push_str(&format!("{} {}", dat.display(), exists_flag(dat)));
            out.push_str(exists_flag(&dat.with_extension("EXT")));
            out.push_str(exists_flag(&dat.with_extension("2EX")));
        }
    }
    out.push('\n');
    out
}

/// The lines of every job that has a failed track, in job order (`ExportDescription::getErrorInfo`).
pub fn entries(jobs: &[Job], results: &[TrackResult]) -> String {
    let mut out = String::new();
    let mut i = 0;
    for job in jobs {
        let mut lines = String::new();
        for step in &job.steps {
            let result = results.get(i).copied().unwrap_or(step.result);
            i += 1;
            lines.push_str(&entry(
                result,
                &step.title,
                &step.location,
                step.analysis_path.as_deref(),
            ));
        }
        out.push_str(&lines);
    }
    out
}

/// UTF-16LE, `\n` → `\r\n`, optionally with a BOM (`juce::File::appendText(…, true, bom, "\r\n")`).
pub fn encode(text: &str, bom: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if bom {
        out.extend_from_slice(&[0xff, 0xfe]);
    }
    for unit in text.replace('\n', "\r\n").encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// Writes (or appends to) today's log in `dir`. Returns the file, or `None` when there is
/// nothing to log. rekordbox appends to an existing log of the same day: it first appends a
/// line end (with a second BOM, as `appendText` writes one), then the entries without a header.
pub fn write_log(
    dir: &Path,
    today: &str,
    strings: &LogStrings,
    body: &str,
) -> std::io::Result<Option<PathBuf>> {
    use std::io::Write;
    if body.is_empty() {
        return Ok(None);
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(log_file_name(today));
    // A new file gets the BOM (written with the first header line) and the header.
    let mut bytes = if path.is_file() {
        encode("\n", true)
    } else {
        encode(&header(strings), true)
    };
    bytes.extend(encode(body, false));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_lines() {
        let h = header(&LogStrings::german());
        assert!(h.starts_with("One or more files were not exported. \nTrack(s)"));
        assert!(h.ends_with("\n\n    Trackname\tLage\n"));
        assert_eq!(
            entry(TrackResult::Unsupported, "t", "/a/t.wav", None),
            "[2] t\t/a/t.wav\n"
        );
        assert_eq!(entry(TrackResult::Exported, "t", "/a", None), "");
        assert_eq!(
            entry(
                TrackResult::AnalysisFailed,
                "t",
                "/a",
                Some(Path::new("/nonexistent/ANLZ0000.DAT"))
            ),
            "[4] t\t/a\t/nonexistent/ANLZ0000.DAT ---\n"
        );
        let enc = encode("a\n", true);
        assert_eq!(enc, [0xff, 0xfe, b'a', 0, b'\r', 0, b'\n', 0]);
    }
}
