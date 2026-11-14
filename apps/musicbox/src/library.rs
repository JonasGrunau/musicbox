//! Background loading of the library snapshot (read-only), on one long-lived thread. Edits reload
//! the whole snapshot; the thread keeps its read connection between loads that follow each other
//! closely, so a reload skips opening master.db again (SQLCipher derives the key on every open).

use crate::Args;
use mb_core::LibrarySnapshot;
use mb_core::key::KeyNotation;
use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

/// A loaded local library plus what the editor needs to open it for writing.
pub struct Loaded {
    pub snapshot: LibrarySnapshot,
    /// `djmdCue` rows by content ID.
    pub cues: std::collections::HashMap<mb_core::DbId, Vec<mb_library::Cue>>,
    pub db_path: PathBuf,
    pub passphrase: String,
    pub analysis_root: PathBuf,
    /// rekordbox's settings folder (MYSETTING*.DAT etc., copied to sticks on export).
    pub settings_root: PathBuf,
    /// rekordbox's language file (options.json `lang-path`).
    pub lang_path: Option<PathBuf>,
    pub prefs: Preferences,
}

pub type LoadResult = Result<Loaded, String>;

/// The rekordbox preferences the app follows, read with the snapshot.
pub struct Preferences {
    /// Key column: `ShowOriginalKey` / `KeyStringSetting`.
    pub key_notation: KeyNotation,
    pub analysis: mb_library::AnalysisSettings,
    /// `AutoAnalysis`: analyse tracks that have no analysis yet (after an import).
    pub auto_analysis: bool,
    /// The rekordbox.xml the tree shows (`xml_bridge::bridge_file`).
    pub bridge_xml: Option<PathBuf>,
}

/// How long an idle read connection stays open. Batch analysis and quick edits reload every
/// second or two; between bursts musicbox holds no handle on master.db.
const KEEP_OPEN: Duration = Duration::from_secs(20);

/// The loader thread: loads once on start and again per [`Loader::reload`].
pub struct Loader {
    /// Dropped first in `drop`, which ends the thread.
    requests: Option<Sender<()>>,
    results: Receiver<LoadResult>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Loader {
    pub fn start(ctx: egui::Context, args: Args) -> Self {
        let (requests, wake) = channel::<()>();
        let (tx, results) = channel();
        let thread = std::thread::Builder::new()
            .name("mb-library-load".into())
            .spawn(move || {
                let mut open = None;
                loop {
                    let result = load(&args, &mut open).map_err(|e| format!("{e:#}"));
                    if tx.send(result).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                    match wake.recv_timeout(KEEP_OPEN) {
                        Ok(()) => {}
                        Err(RecvTimeoutError::Timeout) => {
                            open = None;
                            if wake.recv().is_err() {
                                return;
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    // One load serves every request made before it starts.
                    while wake.try_recv().is_ok() {}
                }
            })
            .expect("spawn library loader");
        Self { requests: Some(requests), results, thread: Some(thread) }
    }

    /// Loads again (after an edit); the result arrives through [`Loader::poll`].
    pub fn reload(&self) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(());
        }
    }

    pub fn poll(&self) -> Option<LoadResult> {
        self.results.try_recv().ok()
    }
}

impl Drop for Loader {
    /// Waits for the thread to close its connection (a running load finishes first): a
    /// connection closing while the process exits crashes in SQLCipher's cleanup.
    fn drop(&mut self) {
        self.requests = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The read connection kept between loads, with what it was opened for.
struct OpenDb {
    db_path: PathBuf,
    passphrase: String,
    /// Device and inode of master.db: a replaced file (a restored backup) is opened again.
    file: (u64, u64),
    db: MasterDb,
}

fn file_id(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn load(args: &Args, open: &mut Option<OpenDb>) -> anyhow::Result<Loaded> {
    let options = args
        .options
        .clone()
        .or_else(LibraryPaths::default_options_json)
        .ok_or_else(|| anyhow::anyhow!("cannot locate rekordboxAgent options.json"))?;
    let mut paths = LibraryPaths::from_options_json(&options)?;
    if let Some(db) = &args.db {
        paths.db_path = db.clone();
    }
    if let Some(root) = &args.analysis_root {
        paths.analysis_data_root = root.clone();
    }
    let passphrase = derive_passphrase(&paths.dp)?;
    let file = file_id(&paths.db_path);
    let reuse = open.as_ref().is_some_and(|o| o.db_path == paths.db_path && o.passphrase == passphrase && Some(o.file) == file);
    if !reuse {
        *open = None;
        let db = MasterDb::open_read_only(&paths.db_path, &passphrase)?;
        let Some(file) = file else { return read(&db, paths, passphrase) };
        *open = Some(OpenDb { db_path: paths.db_path.clone(), passphrase: passphrase.clone(), file, db });
    }
    let result = read(&open.as_ref().expect("opened above").db, paths, passphrase);
    if result.is_err() {
        // Open afresh next time.
        *open = None;
    }
    result
}

fn read(db: &MasterDb, paths: LibraryPaths, passphrase: String) -> anyhow::Result<Loaded> {
    let snapshot = db.load_snapshot(&paths)?;
    let cues = mb_library::cues::load_all(db.connection())?;
    let prefs = Preferences::read(&paths.settings_root);
    Ok(Loaded {
        snapshot,
        cues,
        db_path: paths.db_path,
        passphrase,
        analysis_root: paths.analysis_data_root,
        settings_root: paths.settings_root,
        lang_path: paths.lang_path,
        prefs,
    })
}

impl Preferences {
    /// Reads `rekordbox3.settings` once (each `mb_usb::settings::preference` call reads it again).
    fn read(settings_root: &Path) -> Self {
        let text = std::fs::read_to_string(settings_root.join("rekordbox3.settings")).unwrap_or_default();
        let pref = |name: &str| preference(&text, name);
        Self {
            key_notation: KeyNotation::from_preferences(
                pref("ShowOriginalKey").map(|v| v.trim() != "0"),
                pref("KeyStringSetting").and_then(|v| v.trim().parse().ok()),
            ),
            // Analysis preferences: `AutoGain` and `enableKeyAnalysis` (default on),
            // `trackAnalysisSettingsSongStruct`, `trackAnalysisMode`, `HighPrecisionBeatAnalyze` with
            // `trackHiPrecisionBeatAnalysisMode`, `trackAnalysisSettingsVocalDetect`,
            // `TrackAnalysisSettingsEmbedding` (default on), the CUE analysis preferences
            // (`DetectCue`, `AutoDetectCue`, …) and `cue_personal_trend.json`, `AutoAnalysis`.
            analysis: mb_library::AnalysisSettings {
                auto_gain: pref("AutoGain").is_none_or(|v| v.trim() != "0"),
                phrase: pref("trackAnalysisSettingsSongStruct").is_some_and(|v| v.trim() == "1"),
                key: pref("enableKeyAnalysis").is_none_or(|v| v.trim() != "0"),
                high_precision: mb_library::AnalysisSettings::high_precision_from(
                    pref("HighPrecisionBeatAnalyze").as_deref(),
                    pref("trackHiPrecisionBeatAnalysisMode").as_deref(),
                ),
                dynamic: mb_library::AnalysisSettings::dynamic_from(pref("trackAnalysisMode").as_deref()),
                vocal: pref("trackAnalysisSettingsVocalDetect").is_some_and(|v| v.trim() == "1"),
                embedding: pref("TrackAnalysisSettingsEmbedding").is_none_or(|v| v.trim() != "0"),
                cue: mb_library::CueAnalysisSettings::from_prefs(pref),
                // Auto CUE's personal tendency, read only; an unreadable file counts as none
                cue_trend: mb_library::cue_analysis::read_trend(settings_root).ok().flatten().map(std::sync::Arc::new),
            },
            auto_analysis: pref("AutoAnalysis").is_some_and(|v| v.trim() == "1"),
            bridge_xml: crate::xml_bridge::bridge_file(pref),
        }
    }
}

/// A preference value from the text of `rekordbox3.settings`: the same lookup as
/// `mb_usb::settings::preference`, on text read once.
fn preference(text: &str, name: &str) -> Option<String> {
    let key = format!("name=\"{name}\"");
    let at = text.find(&key)? + key.len();
    let rest = &text[at..text[at..].find('>').map_or(text.len(), |e| at + e)];
    let start = rest.find("val=\"")? + 5;
    Some(rest[start..start + rest[start..].find('"')?].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_matches_mb_usb() {
        let dir = std::env::temp_dir().join(format!("mb-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = "<PROPERTIES>\n<VALUE name=\"AutoGain\" val=\"0\"/>\n<VALUE name=\"KeyStringSetting\" val=\"2\" />\n\
                    <VALUE name=\"Empty\"/>\n<VALUE name=\"bridgeImportedLibraryFile\" val=\"/a b/c.xml\"/>\n</PROPERTIES>\n";
        std::fs::write(dir.join("rekordbox3.settings"), text).unwrap();
        for name in ["AutoGain", "KeyStringSetting", "Empty", "bridgeImportedLibraryFile", "Missing"] {
            assert_eq!(preference(text, name), mb_usb::settings::preference(&dir, name), "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
