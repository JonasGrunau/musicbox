//! The executor: runs an [`ExportPlan`] against the device. Spec: `docs/re/export.md` (algorithm
//! summary, § 1.2, § 3, § 4.5, § 5, § 7 "must"), writer checklist.
//!
//! Order on the device:
//! 1. `PIONEER/rekordbox/`, the "export in progress" markers (`sync.temp`,
//!    `sync.devlibplus.temp`, § 4.5), `DeviceLibBackup/rbDevLibBaInfo_<DBID>.json`, the settings
//!    files (§ 5).
//! 2. Per track, in plan order: audio (bytes only: no extended attributes), stale ANLZ sets
//!    deleted, the USB ANLZ set, artwork. A free-space check (≥ 1.5 × the file) precedes each
//!    copy. The audio is one sequential stream; the ANLZ sets and artwork are written by worker
//!    threads meanwhile, and a background flusher gets everything onto the medium in batches
//!    (`run_tracks`). Existing files are replaced by renames once the new data is flushed.
//! 3. One flush of the whole volume (drive cache included), then the databases:
//!    exportLibrary.db (built in a local temp folder and copied over in one write), export.pdb,
//!    exportExt.pdb, each renamed into place and `F_FULLFSYNC`ed. Only the tracks whose files
//!    were all written are in them.
//! 4. The markers are deleted; the AppleDouble companions of everything written are removed as
//!    it is written ([`crate::appledouble::Cleaner`]); the export log is written for tracks that
//!    were not exported.
//!
//! Cancellation or a failed copy stops new tracks (the ones in flight finish); the databases are
//! still written for the tracks done so far, so the stick stays consistent.

use crate::device::{DeviceState, free_bytes};
use crate::log::{LogStrings, entries, write_log};
use crate::naming::pioneer_dir;
use crate::appledouble::Cleaner;
use crate::plan::{
    ArtworkSize, DevlibChange, DevlibOp, ExportPlan, ExtOp, FileOp, PdbLookup, PdbOp, TrackResult,
    TrackStep, blank_pdb_track,
};
use crate::settings::settings_writes;
use crate::source::LocalArtwork;
use crate::{Error, Result};
use mb_devlib::{CreateOptions, DevLibWriter, ImageOwner, Lookup, LookupColumn};
use mb_pdb::engine::{Db, RowRef};
use mb_pdb::schema::DbKind;
use mb_pdb::tables::{Artwork, MyTag, MyTagTrack, TypedRow};
use mb_pdb::writer::{Exporter, Lookups, insert_row, locate_rows};
use mb_pdb::{ExtDb, Pdb};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};

/// Cancels a running export at the next track (or the next 1 MiB of audio).
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Prepare,
    Tracks,
    Databases,
    Finish,
}

/// Progress of an export.
#[derive(Debug, Clone)]
pub struct Progress {
    pub phase: Phase,
    /// Tracks processed / total tracks of the plan.
    pub tracks_done: usize,
    pub tracks_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Title of the track being exported.
    pub current: Option<String>,
}

/// Executor settings.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// The local settings folder (rekordbox: `~/Library/Application Support/Pioneer/rekordbox6`)
    /// with `MYSETTING.DAT`, `MYSETTING2.DAT`, `DJMMYSETTING.DAT`, `DEVSETTING.DAT` and
    /// `djprofile.nxs`. `None`: the defaults, and no `djprofile.nxs`.
    pub settings_dir: Option<PathBuf>,
    /// rekordbox's preference "apply My Settings to devices" (§ 5). Default on.
    pub apply_my_settings: bool,
    /// Folder for the export log (rekordbox: `~/Documents/rekordbox`). `None`: no log file.
    pub log_dir: Option<PathBuf>,
    pub log_strings: LogStrings,
    /// Write `DeviceLibBackup/rbDevLibBaInfo_<DBID>.json` when missing (§ 6.3 step 4).
    pub write_backup_info: bool,
    /// Check free space before each copy (≥ 1.5 × the file, § 1.2 step 7.1).
    pub check_free_space: bool,
    /// Replace the plan's export.pdb / exportExt.pdb operation lists (for replaying a recorded
    /// session, e.g. the oracle stick's reopen cycles). Ignores cancellation.
    pub pdb_ops: Option<Vec<PdbOp>>,
    pub ext_ops: Option<Vec<ExtOp>>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            settings_dir: None,
            apply_my_settings: true,
            log_dir: None,
            log_strings: LogStrings::default(),
            write_backup_info: true,
            check_free_space: true,
            pdb_ops: None,
            ext_ops: None,
        }
    }
}

/// What an export did.
#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    /// The result of every planned step, in plan order (steps not reached keep the planned
    /// result).
    pub results: Vec<TrackResult>,
    /// Steps whose files were written (all of them unless cancelled or failed).
    pub completed: usize,
    pub cancelled: bool,
    /// The first error that stopped the track phase.
    pub error: Option<String>,
    pub log_file: Option<PathBuf>,
}

impl ExportReport {
    /// Tracks that are on the device after the export (exported or already there), among the
    /// steps that ran.
    pub fn exported(&self) -> usize {
        self.results[..self.completed.min(self.results.len())]
            .iter()
            .filter(|r| r.is_success())
            .count()
    }
}

struct Ctx<'a> {
    root: &'a Path,
    cancel: &'a CancelToken,
    progress: &'a mut dyn FnMut(&Progress),
    state: Progress,
    check_space: bool,
}

impl Ctx<'_> {
    fn fs(&self, device_path: &str) -> PathBuf {
        self.root.join(device_path.trim_start_matches('/'))
    }
    fn report(&mut self) {
        (self.progress)(&self.state);
    }
}

/// Runs `plan` on the device it was made for. `dev` must be the state the plan was made from.
pub fn execute(
    plan: &ExportPlan,
    dev: &DeviceState,
    opts: &ExportOptions,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(&Progress),
) -> Result<ExportReport> {
    let root = plan.root.as_path();
    let pioneer = root.join(pioneer_dir(plan.hidden));
    let rb = pioneer.join("rekordbox");
    mkdirs(&rb)?;
    let steps: Vec<_> = plan.steps().collect();
    let cleaner = Cleaner::new(root, dev.filesystem.as_deref());
    // Written outside the track phase; their AppleDouble companions are removed at the end.
    let mut side_files = vec![rb.clone()];
    let mut ctx = Ctx {
        root,
        cancel,
        progress,
        state: Progress {
            phase: Phase::Prepare,
            tracks_done: 0,
            tracks_total: steps.len(),
            bytes_done: 0,
            bytes_total: plan.bytes_to_copy(),
            current: None,
        },
        check_space: opts.check_free_space,
    };
    ctx.report();
    check_unchanged(dev, &rb)?;

    // Markers: `exportPlaylist` creates them when the jobs are built.
    let has_lists = plan.jobs.iter().any(|j| j.master_playlist_id.is_some());
    let markers = [rb.join("sync.temp"), rb.join("sync.devlibplus.temp")];
    if has_lists {
        for m in &markers {
            write_new(m, b"")?;
        }
    }
    if opts.write_backup_info {
        let dir = pioneer.join("DeviceLibBackup");
        let file = dir.join(format!("rbDevLibBaInfo_{}.json", plan.source_dbid));
        if !file.exists() {
            mkdirs(&dir)?;
            let uuid = random_hex32()?;
            write_new(
                &file,
                format!("{{\r\n  \"uuid\": \"{uuid}\",\r\n  \"info\": []\r\n}}").as_bytes(),
            )?;
            side_files.push(file);
        }
    }
    for (path, bytes) in settings_writes(
        opts.settings_dir.as_deref(),
        &pioneer,
        opts.apply_my_settings,
    ) {
        write_replace(&path, &bytes)?;
        side_files.push(path);
    }
    // Device Library Plus conversion: `a` artwork copied to `b` (§ 6.4; the database itself is
    // built with the others at the end).
    if let Some(conv) = &plan.devlib_convert {
        side_files.extend(crate::convert::copy_artwork(root, conv)?);
    }

    // Tracks.
    let mut report = ExportReport {
        results: steps.iter().map(|s| s.result).collect(),
        ..Default::default()
    };
    ctx.state.phase = Phase::Tracks;
    let tracks = run_tracks(&mut ctx, &steps, &cleaner);
    report.completed = tracks.completed;
    report.cancelled = tracks.cancelled;
    if let Some((i, result, e)) = tracks.failed {
        report.results[i] = result;
        report.error = Some(e.to_string());
    }
    ctx.state.current = None;
    ctx.state.tracks_done = report.completed;

    // Databases last (`run_tracks` ended with the volume flushed, drive cache included).
    ctx.state.phase = Phase::Databases;
    ctx.report();
    let completed = Some(report.completed);
    write_devlib(plan, &rb, completed)?;
    let pdb_ops = opts
        .pdb_ops
        .clone()
        .unwrap_or_else(|| plan.pdb_ops(completed));
    write_pdb(plan, dev, &rb, &pdb_ops)?;
    let ext_ops = opts
        .ext_ops
        .clone()
        .unwrap_or_else(|| plan.ext_ops(completed));
    write_ext(plan, dev, &rb, &ext_ops)?;
    side_files.extend(["exportLibrary.db", "export.pdb", "exportExt.pdb"].map(|n| rb.join(n)));

    ctx.state.phase = Phase::Finish;
    ctx.report();
    if has_lists {
        for m in &markers {
            let _ = std::fs::remove_file(m);
        }
    }
    for path in &side_files {
        cleaner.clean(path);
    }
    full_fsync_dir(&rb);
    if let Some(dir) = &opts.log_dir {
        let body = entries(&plan.jobs, &report.results);
        report.log_file =
            write_log(dir, &plan.today, &opts.log_strings, &body).map_err(|e| Error::io(dir, e))?;
    }
    ctx.report();
    Ok(report)
}

/// The databases on the device must still be the ones the plan was made from.
fn check_unchanged(dev: &DeviceState, rb: &Path) -> Result<()> {
    let now = std::fs::read(rb.join("export.pdb")).ok();
    let then = dev.pdb.as_ref().map(|p| &p.export_bytes);
    if now.as_ref() != then {
        return Err(Error::Device(
            "export.pdb changed since the export was planned".into(),
        ));
    }
    if dev.devlib.is_some() != rb.join("exportLibrary.db").is_file() {
        return Err(Error::Device(
            "exportLibrary.db changed since the export was planned".into(),
        ));
    }
    Ok(())
}

// ---- tracks ---------------------------------------------------------------------------------

/// The flusher syncs the volume once this much is written since its last sync.
const FLUSH_EVERY: u64 = 32 << 20;

/// How far the copy may run ahead of the device: written bytes not yet flushed (the page cache
/// takes writes at memory speed, so without a limit the progress would be meaningless and the
/// whole export would sit in RAM). More than [`FLUSH_EVERY`], so the flusher always has a batch
/// when the copy waits for it.
const MAX_UNFLUSHED: u64 = 128 << 20;

const ANLZ_EXTS: [&str; 3] = ["DAT", "EXT", "2EX"];

enum StepError {
    Cancelled,
    Failed(TrackResult, Error),
}

/// The outcome of the track phase.
struct Tracks {
    /// Steps whose files are all written and on the medium: a prefix of the plan.
    completed: usize,
    cancelled: bool,
    /// The first step that failed, its result and the error.
    failed: Option<(usize, TrackResult, Error)>,
}

/// Work for the small-file threads.
enum Job<'a> {
    /// The ANLZ and artwork operations of one step.
    Files { step: usize, ops: Vec<&'a FileOp> },
    /// Remove the AppleDouble companions of a file written by the calling thread (audio).
    Clean(PathBuf),
}

/// What one step's small-file job wrote.
#[derive(Default)]
struct Written {
    /// Every file written (new files at their final path, replacements under their temporary
    /// name), for the `fsync` fallback of the flusher.
    files: Vec<PathBuf>,
    bytes: u64,
    /// Replacements of existing files: (temporary, final). Renamed into place only after the
    /// last flush, so the old file stays whole until the new one is on the medium.
    renames: Vec<(PathBuf, PathBuf)>,
}

type JobResult = (usize, std::result::Result<Written, (TrackResult, Error)>);

/// Files the flusher must get onto the medium, for one step.
struct Flush {
    step: usize,
    files: Vec<PathBuf>,
    bytes: u64,
}

/// The flusher's progress, shared with the copying thread.
#[derive(Default)]
struct Flushed {
    state: Mutex<FlushState>,
    changed: Condvar,
}

#[derive(Default)]
struct FlushState {
    bytes: u64,
    /// The lowest step whose files could not be flushed.
    failed: Option<(usize, Error)>,
    /// The flusher has stopped.
    done: bool,
}

impl Flushed {
    fn lock(&self) -> std::sync::MutexGuard<'_, FlushState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn bytes(&self) -> u64 {
        self.lock().bytes
    }
    /// Blocks until at least `bytes` are flushed (or the flusher has stopped).
    fn wait_for(&self, bytes: u64) {
        let state = self.lock();
        let _unused = self
            .changed
            .wait_while(state, |s| s.bytes < bytes && !s.done)
            .unwrap_or_else(|e| e.into_inner());
    }
    fn update(&self, f: impl FnOnce(&mut FlushState)) {
        f(&mut self.lock());
        self.changed.notify_all();
    }
}

/// Bookkeeping of the copying thread.
struct TrackState {
    /// Per step: its small files are written (or it has none).
    small_done: Vec<bool>,
    /// Steps whose audio is written and whose small-file job is queued.
    queued: usize,
    /// Steps whose files are all written: a prefix.
    written: usize,
    /// Bytes handed to the flusher.
    requested: u64,
    failed: Option<(usize, TrackResult, Error)>,
    cancelled: bool,
    /// Audio files written, by step.
    audio: Vec<(usize, PathBuf)>,
    /// Deferred replacements, by step.
    renames: Vec<(usize, PathBuf, PathBuf)>,
}

impl TrackState {
    fn fail(&mut self, step: usize, result: TrackResult, e: Error) {
        if self.failed.as_ref().is_none_or(|(i, ..)| step < *i) {
            self.failed = Some((step, result, e));
        }
    }

    fn flush(&mut self, tx: &mpsc::Sender<Flush>, step: usize, files: Vec<PathBuf>, bytes: u64) {
        self.requested += bytes;
        let _ = tx.send(Flush { step, files, bytes });
    }

    fn job_done(&mut self, (step, result): JobResult, tx: &mpsc::Sender<Flush>) {
        match result {
            Ok(w) => {
                self.small_done[step] = true;
                self.renames
                    .extend(w.renames.into_iter().map(|(tmp, path)| (step, tmp, path)));
                self.flush(tx, step, w.files, w.bytes);
            }
            Err((result, e)) => self.fail(step, result, e),
        }
    }

    fn advance(&mut self) {
        while self.written < self.queued && self.small_done[self.written] {
            self.written += 1;
        }
    }
}

/// Writes the files of every step: the audio on this thread, the ANLZ sets and artwork on one
/// other thread meanwhile (in plan order), all flushed to the medium in the background by
/// [`flusher`]. More writer threads do not help on a real stick: FSKit FAT handles one request
/// at a time there, and parallel requests only add contention (RBTEST, 30 tracks, 302 MB:
/// 61 s with one writer thread, 71 s with four, 76 s for the old sequential executor; on a
/// FAT32 disk image four threads were faster, which misled). The audio stays one sequential
/// stream, so the FAT allocator keeps each audio file contiguous. A step counts once all its
/// files are written; the first failure or a cancel stops new steps and the
/// ones in flight finish. Ends with every counted step on the medium: the deferred renames and
/// one flush of the whole volume, drive cache included. Audio and replacements of steps that do
/// not count are removed again; their new ANLZ sets and artwork stay unreferenced, as after an
/// interrupted export.
fn run_tracks(ctx: &mut Ctx, steps: &[&TrackStep], cleaner: &Cleaner) -> Tracks {
    let root = ctx.root;
    let flushed = Flushed::default();
    let mut t = TrackState {
        small_done: vec![false; steps.len()],
        queued: 0,
        written: 0,
        requested: 0,
        failed: None,
        cancelled: false,
        audio: Vec::new(),
        renames: Vec::new(),
    };
    std::thread::scope(|s| {
        let (job_tx, job_rx) = mpsc::sync_channel::<Job>(4);
        let (done_tx, done_rx) = mpsc::channel::<JobResult>();
        s.spawn(move || small_file_thread(root, job_rx, done_tx, cleaner));
        let (flush_tx, flush_rx) = mpsc::channel::<Flush>();
        let flushed = &flushed;
        s.spawn(move || flusher(root, flush_rx, flushed));
        // Takes in finished small-file jobs; called per step and per 1 MiB of audio.
        let poll = |t: &mut TrackState, progress: &mut Progress| {
            for r in done_rx.try_iter() {
                t.job_done(r, &flush_tx);
            }
            t.advance();
            progress.tracks_done = t.written;
        };

        for (i, step) in steps.iter().enumerate() {
            poll(&mut t, &mut ctx.state);
            if t.failed.is_some() {
                break;
            }
            if ctx.cancel.is_cancelled() {
                t.cancelled = true;
                break;
            }
            ctx.state.current = Some(step.title.clone());
            ctx.report();
            let mut error = None;
            for f in &step.files {
                let FileOp::CopyAudio { from, to, size } = f else {
                    continue;
                };
                let dest = ctx.fs(to);
                flushed.wait_for(t.requested.saturating_sub(MAX_UNFLUSHED));
                let unflushed = t.requested.saturating_sub(flushed.bytes());
                let mut copied = Ok(());
                if ctx.check_space {
                    copied = check_space(ctx.root, &dest, *size, unflushed);
                }
                if copied.is_ok() {
                    copied = copy_audio(ctx, from, &dest, &mut |p| poll(&mut t, p));
                }
                match copied {
                    Ok(()) => {
                        t.audio.push((i, dest.clone()));
                        t.flush(&flush_tx, i, vec![dest.clone()], *size);
                        let _ = job_tx.send(Job::Clean(dest));
                    }
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                }
            }
            match error {
                Some(StepError::Cancelled) => {
                    t.cancelled = true;
                    break;
                }
                Some(StepError::Failed(result, e)) => {
                    t.fail(i, result, e);
                    break;
                }
                None => {}
            }
            let ops: Vec<&FileOp> = step
                .files
                .iter()
                .filter(|f| !matches!(f, FileOp::CopyAudio { .. }))
                .collect();
            if ops.is_empty() {
                t.small_done[i] = true;
            } else {
                let _ = job_tx.send(Job::Files { step: i, ops });
            }
            t.queued = i + 1;
        }
        // No new steps; the jobs in flight finish.
        drop(job_tx);
        for r in done_rx.iter() {
            t.job_done(r, &flush_tx);
        }
        t.advance();
        drop(flush_tx);
    });
    if let Some((step, e)) = flushed.lock().failed.take() {
        t.fail(step, TrackResult::CopyFailed, e);
    }
    let completed = t
        .failed
        .as_ref()
        .map_or(t.written, |(i, ..)| t.written.min(*i));

    // Files of steps that do not count are not in the databases: not left behind.
    for (i, path) in &t.audio {
        if *i >= completed {
            let _ = std::fs::remove_file(path);
        }
    }
    let (renames, dropped): (Vec<_>, Vec<_>) =
        t.renames.into_iter().partition(|(i, ..)| *i < completed);
    for (_, tmp, _) in dropped {
        let _ = std::fs::remove_file(tmp);
    }
    // The replacements, now that their data is on the medium.
    let completed = match rename_all(&renames, cleaner) {
        Some((i, e)) => {
            if t.failed.as_ref().is_none_or(|(j, ..)| i < *j) {
                t.failed = Some((i, TrackResult::CopyFailed, e));
            }
            completed.min(i)
        }
        None => completed,
    };
    if sync_volume(root, true).is_err() {
        full_fsync_dir(root);
    }
    Tracks {
        completed,
        cancelled: t.cancelled,
        failed: t.failed,
    }
}

fn small_file_thread(
    root: &Path,
    jobs: mpsc::Receiver<Job>,
    done: mpsc::Sender<JobResult>,
    cleaner: &Cleaner,
) {
    for job in jobs {
        match job {
            Job::Clean(path) => cleaner.clean(&path),
            Job::Files { step, ops } => {
                if done.send((step, small_files(root, &ops, cleaner))).is_err() {
                    return;
                }
            }
        }
    }
}

/// One step's ANLZ and artwork operations. A failed ANLZ set is
/// [`TrackResult::AnalysisFailed`], failed artwork [`TrackResult::CopyFailed`].
fn small_files(
    root: &Path,
    ops: &[&FileOp],
    cleaner: &Cleaner,
) -> std::result::Result<Written, (TrackResult, Error)> {
    let fs = |p: &str| root.join(p.trim_start_matches('/'));
    // Parts this step writes again are replaced rather than deleted first, so the old ones stay
    // whole until the new ones are on the medium.
    let mut rewritten = HashSet::new();
    for op in ops {
        if let FileOp::WriteAnlz { dat, parts, .. } = op {
            for (ext, _) in ANLZ_EXTS.iter().zip(parts).filter(|(_, p)| **p) {
                rewritten.insert(fs(dat).with_extension(ext));
            }
        }
    }
    let mut out = Written::default();
    // `a` and `b` files of one image are the same bytes (§ 3): read or made once.
    let mut artwork: HashMap<(&Path, u32), Vec<u8>> = HashMap::new();
    for op in ops {
        match op {
            FileOp::CopyAudio { .. } => {}
            FileOp::DeleteAnlz { dat } => {
                for ext in ANLZ_EXTS {
                    let p = fs(dat).with_extension(ext);
                    if !rewritten.contains(&p) {
                        let _ = std::fs::remove_file(p);
                    }
                }
            }
            FileOp::WriteAnlz {
                local_dat,
                parts,
                dat,
                ppth,
                cues,
            } => {
                let fail = |e| (TrackResult::AnalysisFailed, e);
                for (ext, _) in ANLZ_EXTS.iter().zip(parts).filter(|(_, p)| **p) {
                    let bytes = usb_anlz(&local_dat.with_extension(ext), ext, ppth, cues)
                        .map_err(fail)?;
                    put(&fs(dat).with_extension(ext), &bytes, &mut out, cleaner).map_err(fail)?;
                }
            }
            FileOp::Artwork { from, size, to } => {
                let fail = |e| (TrackResult::CopyFailed, e);
                let bytes = match artwork.entry((from.base.as_path(), size.pixels())) {
                    Entry::Occupied(e) => e.into_mut(),
                    Entry::Vacant(e) => e.insert(artwork_bytes(from, *size).map_err(fail)?),
                };
                put(&fs(to), bytes, &mut out, cleaner).map_err(fail)?;
            }
        }
    }
    Ok(out)
}

/// Writes `bytes` to `path` without waiting for the device (the flusher does that). A new file
/// is written in place; an existing one is written next to it and renamed into place after the
/// last flush ([`Written::renames`]), like [`write_replace`]. A failed write leaves no partial
/// file behind.
fn put(path: &Path, bytes: &[u8], out: &mut Written, cleaner: &Cleaner) -> Result<()> {
    if let Some(dir) = path.parent() {
        mkdirs(dir)?;
    }
    let new = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path);
    let (target, mut file, replace) = match new {
        Ok(f) => (path.to_path_buf(), f, false),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let tmp = temp_path(path);
            let f = std::fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            (tmp, f, true)
        }
        Err(e) => return Err(Error::io(path, e)),
    };
    if let Err(e) = file.write_all(bytes) {
        drop(file);
        let _ = std::fs::remove_file(&target);
        return Err(Error::io(&target, e));
    }
    out.files.push(target.clone());
    out.bytes += bytes.len() as u64;
    if replace {
        out.renames.push((target, path.to_path_buf()));
    } else {
        cleaner.clean(path);
    }
    Ok(())
}

/// Renames the deferred replacements into place, in plan order, up to the first failure (the
/// temporary files from there on are removed). Returns the step that failed.
fn rename_all(
    renames: &[(usize, PathBuf, PathBuf)],
    cleaner: &Cleaner,
) -> Option<(usize, Error)> {
    let mut failed = None;
    for (i, tmp, path) in renames {
        if failed.is_none() {
            match std::fs::rename(tmp, path) {
                Ok(()) => {
                    cleaner.clean(path);
                    continue;
                }
                Err(e) => failed = Some((*i, Error::io(path, e))),
            }
        }
        let _ = std::fs::remove_file(tmp);
    }
    failed
}

/// Gets written files onto the medium in the background: one [`sync_volume`] per
/// [`FLUSH_EVERY`] bytes written, and one at the end. The copy never waits for the device
/// between files, and the file system's metadata goes out once per batch instead of once per
/// file. Each sync slows the file system for the other threads (FSKit FAT, small-file creates:
/// 2 ms, 6 ms during volume syncs, 20 ms during per-file `fsync`s; measured 2026-10-15), so
/// it runs no more often than that. On a file system that refuses it: `fsync` per file.
fn flusher(root: &Path, requests: mpsc::Receiver<Flush>, flushed: &Flushed) {
    // Marks the flusher stopped however it ends, so `Flushed::wait_for` never waits for nothing.
    struct Stopped<'a>(&'a Flushed);
    impl Drop for Stopped<'_> {
        fn drop(&mut self) {
            self.0.update(|s| s.done = true);
        }
    }
    let _stopped = Stopped(flushed);
    let mut batch: Vec<Flush> = Vec::new();
    let mut open = true;
    while open {
        match requests.recv() {
            Ok(f) => batch.push(f),
            Err(_) => open = false,
        }
        batch.extend(requests.try_iter());
        let bytes: u64 = batch.iter().map(|f| f.bytes).sum();
        if batch.is_empty() || open && bytes < FLUSH_EVERY {
            continue;
        }
        let mut failed = None;
        if sync_volume(root, false).is_err() {
            'batch: for f in &batch {
                for p in &f.files {
                    if let Err(e) = std::fs::File::open(p).and_then(|file| fsync(&file)) {
                        failed = Some((f.step, Error::io(p, e)));
                        break 'batch;
                    }
                }
            }
        }
        batch.clear();
        flushed.update(|s| {
            s.bytes += bytes;
            if let Some((i, e)) = failed
                && s.failed.as_ref().is_none_or(|(j, _)| i < *j)
            {
                s.failed = Some((i, e));
            }
        });
    }
}

/// rekordbox's free-space rule: at least 1.5 × the file (§ 1.2 step 7.1). FSKit FAT counts
/// written data as used only once it is flushed (checked 2026-10-15), so `unflushed` bytes are
/// taken off.
fn check_space(
    root: &Path,
    dest: &Path,
    size: u64,
    unflushed: u64,
) -> std::result::Result<(), StepError> {
    let parent = dest.parent().unwrap_or(root);
    let probe = if parent.exists() { parent } else { root };
    match free_bytes(probe) {
        Some(0) => Err(StepError::Failed(
            TrackResult::DeviceGone,
            Error::Device("the volume reports no space".into()),
        )),
        Some(free) if (free.saturating_sub(unflushed) as f64) < 1.5 * (size as f64) => {
            Err(StepError::Failed(
                TrackResult::NoSpace,
                Error::Device(format!(
                    "{free} bytes free ({unflushed} not yet flushed), {size} needed × 1.5"
                )),
            ))
        }
        _ => Ok(()),
    }
}

/// Copies the audio bytes only (no extended attributes or resource forks, which macOS would
/// store as `._` AppleDouble files on FAT), 1 MiB at a time, checking for cancellation. Not
/// flushed here: the flusher does that in the background. `poll` updates the progress before
/// each report.
fn copy_audio(
    ctx: &mut Ctx,
    from: &Path,
    dest: &Path,
    poll: &mut dyn FnMut(&mut Progress),
) -> std::result::Result<(), StepError> {
    let failed = |e: Error| StepError::Failed(TrackResult::CopyFailed, e);
    if let Some(dir) = dest.parent() {
        mkdirs(dir).map_err(failed)?;
    }
    let mut src = std::fs::File::open(from).map_err(|e| failed(Error::io(from, e)))?;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .map_err(|e| failed(Error::io(dest, e)))?;
    let mut buf = vec![0u8; 1 << 20];
    let result = (|| -> std::result::Result<(), StepError> {
        loop {
            if ctx.cancel.is_cancelled() {
                return Err(StepError::Cancelled);
            }
            let n = src.read(&mut buf).map_err(|e| failed(Error::io(from, e)))?;
            if n == 0 {
                return Ok(());
            }
            out.write_all(&buf[..n])
                .map_err(|e| failed(Error::io(dest, e)))?;
            ctx.state.bytes_done += n as u64;
            poll(&mut ctx.state);
            ctx.report();
        }
    })();
    if result.is_err() {
        drop(out);
        let _ = std::fs::remove_file(dest);
    }
    result
}

/// One file of the USB ANLZ set, made from the local one (`ExportAnalysisData*`,
/// `docs/re/anlz.md` § Local vs USB): PPTH = device path, PSSI/PVDI scrambled, the cue lists from
/// master.db (`.DAT` and `.EXT`), everything else unchanged. `.3EX` is never exported.
fn usb_anlz(src: &Path, ext: &str, ppth: &str, cues: &crate::cues::DeviceCues) -> Result<Vec<u8>> {
    let bytes = std::fs::read(src).map_err(|e| Error::io(src, e))?;
    let file = mb_anlz::AnlzFile::parse(&bytes).map_err(|source| Error::Anlz {
        path: src.to_path_buf(),
        source,
    })?;
    let mut usb = mb_anlz::usb::to_usb(&file, ppth);
    match ext {
        "DAT" => crate::cues::apply_dat(&mut usb, cues),
        "EXT" => crate::cues::apply_ext(&mut usb, cues),
        _ => {}
    }
    Ok(usb.to_bytes())
}

/// The bytes of one device artwork file: the local variant as is, or — when the variant is
/// missing (`create_small_arts_if_not_exists`) — made from the base image the way rekordbox
/// makes it (`crate::artwork`: centred on an opaque black square, scaled, JUCE's JPEG
/// settings; § 3).
fn artwork_bytes(art: &LocalArtwork, size: ArtworkSize) -> Result<Vec<u8>> {
    let variant = match size {
        ArtworkSize::Small => &art.small,
        ArtworkSize::Medium => &art.medium,
        ArtworkSize::Large => &art.large,
    };
    if let Some(p) = variant {
        return std::fs::read(p).map_err(|e| Error::io(p, e));
    }
    let fail = |message: String| Error::Artwork {
        path: art.base.clone(),
        message,
    };
    let base = std::fs::read(&art.base).map_err(|e| Error::io(&art.base, e))?;
    crate::artwork::square_jpeg(&base, size.pixels()).map_err(fail)
}

// ---- databases ------------------------------------------------------------------------------

/// Builds exportLibrary.db in a local temp folder and puts it on the device with one sequential
/// write: SQLite's page writes and the WAL checkpoint are small random writes, which USB sticks
/// are slowest at, and an existing file is read from the stick once instead of copied
/// stick-to-stick. Renamed into place as before; nothing is left on the stick on failure.
fn write_devlib(plan: &ExportPlan, rb: &Path, completed: Option<usize>) -> Result<()> {
    let target = rb.join("exportLibrary.db");
    let local = LocalDir::new("devlib")?;
    let db = local.path().join("exportLibrary.db");
    let writer = match (&plan.devlib_create, &plan.devlib_convert) {
        (Some(create), _) => DevLibWriter::create(
            &db,
            &CreateOptions {
                created_date: Some(create.created_date.clone()),
                menus: create.menus.clone(),
                ..CreateOptions::default()
            },
        )?,
        // Device Library Plus conversion of an old stick (§ 6.4), then the export's operations.
        (None, Some(conv)) => {
            let w = DevLibWriter::create_for_conversion(
                &db,
                &CreateOptions {
                    created_date: Some(conv.today.clone()),
                    menus: conv.menus.clone(),
                    ..CreateOptions::default()
                },
            )?;
            conv.write(&w)?;
            w
        }
        (None, None) => {
            copy_file(&target, &db)?;
            DevLibWriter::open(&db)?
        }
    };
    let mut writer = writer;
    writer.set_today(&plan.today);
    writer.begin()?;
    for op in plan.devlib_ops(completed) {
        apply_devlib(&writer, &op, plan.hidden)?;
    }
    writer.commit()?;
    writer.close()?;
    put_devlib(&db, rb)
}

/// Changes the device's exportLibrary.db (in `rb`) with `edit`, on a local copy like
/// [`write_devlib`]: one sequential read and one sequential write on the stick.
pub(crate) fn edit_devlib(rb: &Path, edit: impl FnOnce(&DevLibWriter) -> Result<()>) -> Result<()> {
    let local = LocalDir::new("devlib")?;
    let db = local.path().join("exportLibrary.db");
    copy_file(&rb.join("exportLibrary.db"), &db)?;
    let writer = DevLibWriter::open(&db)?;
    edit(&writer)?;
    writer.close()?;
    put_devlib(&db, rb)
}

/// Puts the closed database `db` on the device as `rb/exportLibrary.db`: written next to it,
/// `F_FULLFSYNC`ed, renamed into place.
fn put_devlib(db: &Path, rb: &Path) -> Result<()> {
    let target = rb.join("exportLibrary.db");
    let tmp = rb.join(".exportLibrary.db.musicbox");
    remove_db_files(&tmp);
    copy_file(db, &tmp)?;
    full_fsync_file(&tmp)?;
    std::fs::rename(&tmp, &target).map_err(|e| Error::io(&target, e))?;
    full_fsync_file(&target)
}

/// A folder in the local temp directory, removed with its contents when dropped.
pub(crate) struct LocalDir(PathBuf);

impl LocalDir {
    pub(crate) fn new(what: &str) -> Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "mb-usb-{what}-{}-{nanos}",
            std::process::id()
        ));
        mkdirs(&dir)?;
        Ok(Self(dir))
    }
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for LocalDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn remove_db_files(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

fn check(what: &'static str, got: u32, expected: u32) -> Result<()> {
    if got == expected {
        Ok(())
    } else {
        Err(Error::IdMismatch {
            what,
            got,
            expected,
        })
    }
}

/// One exportLibrary.db operation (`docs/re/ops/export-devlib.md` phases 3–5).
pub fn apply_devlib(w: &DevLibWriter, op: &DevlibOp, hidden: bool) -> Result<()> {
    match op {
        DevlibOp::WriteMyTags {
            master_db_id,
            rows,
            links,
        } => {
            w.write_my_tags(*master_db_id, rows)?;
            w.clear_my_tag_content()?;
            for (content, tags) in links {
                w.add_my_tags_to_track(*content, tags)?;
            }
        }
        DevlibOp::CreatePlaylist {
            expect_id,
            name,
            parent,
            folder,
            position,
        } => {
            let id = w.create_playlist(name, *parent, *folder, *position)?;
            check("playlist_id", id, *expect_id)?;
        }
        DevlibOp::ClearPlaylist(id) => w.clear_playlist(*id)?,
        DevlibOp::AddTrack {
            expect_id,
            track,
            image,
            my_tags,
        } => {
            let id = w.add_track(track)?;
            check("content_id", id, *expect_id)?;
            if let Some(expect) = image {
                let (img, _) = w.add_image(ImageOwner::Content(id), hidden)?;
                check("image_id", img, *expect)?;
            }
            if !my_tags.is_empty() {
                w.add_my_tags_to_track(id, my_tags)?;
            }
        }
        DevlibOp::ModifyTrack {
            content_id,
            changes,
        } => {
            for change in changes {
                match change {
                    DevlibChange::Column(column, value) => {
                        w.modify_column(*content_id, *column, value.clone())?
                    }
                    DevlibChange::Lookup(column, name) => {
                        let lookup = match column {
                            LookupColumn::Genre => Lookup::Genre,
                            LookupColumn::Label => Lookup::Label,
                            LookupColumn::Key => Lookup::Key,
                            _ => Lookup::Artist,
                        };
                        let id = w.get_or_add(lookup, name)?;
                        w.connection().execute(
                            &format!(
                                "update content set {} = ?1 where content_id = ?2",
                                column.name()
                            ),
                            rusqlite::params![id, content_id],
                        )?;
                    }
                    DevlibChange::Album {
                        name,
                        compilation,
                        album_artist,
                    } => {
                        let id = w.get_or_add_album(name, *compilation, album_artist)?;
                        w.connection().execute(
                            "update content set album_id = ?1 where content_id = ?2",
                            rusqlite::params![id, content_id],
                        )?;
                    }
                }
            }
        }
        DevlibOp::SetTrackUpdated { content_id, marker } => {
            w.set_track_updated(*content_id, marker)?
        }
        DevlibOp::SetAnalysisUpdated {
            content_id,
            analysed,
            analysis_marker,
            cue_marker,
        } => w.set_analysis_updated(*content_id, *analysed, analysis_marker, cue_marker)?,
        DevlibOp::SetContentLink {
            content_id,
            content_link,
        } => w.set_content_link(*content_id, *content_link)?,
        DevlibOp::AddImage {
            content_id,
            expect_id,
        } => {
            let (img, _) = w.add_image(ImageOwner::Content(*content_id), hidden)?;
            check("image_id", img, *expect_id)?;
        }
        DevlibOp::AppendEntry {
            playlist_id,
            content_id,
        } => w.append_playlist_entry(*playlist_id, *content_id)?,
    }
    Ok(())
}

/// Builds export.pdb by running `ops` (`docs/re/pdb.md` § Export operation sequence).
pub fn build_pdb(
    create: Option<&mb_pdb::ExportDb>,
    existing: Option<&[u8]>,
    ops: &[PdbOp],
    lookups: &Lookups,
) -> Result<Vec<u8>> {
    let mut ex = match (create, existing) {
        (Some(rows), _) => Exporter::create(rows)?,
        (None, Some(bytes)) => Exporter::load(bytes)?,
        (None, None) => return Err(Error::Plan("no export.pdb to continue".into())),
    };
    for op in ops {
        apply_pdb(&mut ex, op, lookups)?;
    }
    Ok(ex.finish())
}

fn apply_pdb(ex: &mut Exporter, op: &PdbOp, l: &Lookups) -> Result<()> {
    match op {
        PdbOp::CreateFirstPlaylist(n) => ex.create_first_playlist(n)?,
        PdbOp::CreatePlaylistAtTop(n) => ex.create_playlist_at_top(n)?,
        PdbOp::InsertPlaylist(n) => ex.insert_playlist(n)?,
        PdbOp::UpdatePlaylists(v) => ex.update_playlists(v)?,
        PdbOp::TouchPlaylist(id) => ex.touch_playlist(*id)?,
        PdbOp::ClearPlaylist(id) => ex.clear_playlist(*id)?,
        PdbOp::AddLookup(k) => {
            // `Exporter::add_lookups` inserts each missing referenced row in its own
            // transaction; a track that references only this row inserts exactly it.
            let mut t = blank_pdb_track();
            match k {
                PdbLookup::Artist(id) => t.artist_id = *id,
                PdbLookup::Genre(id) => t.genre_id = *id,
                PdbLookup::Album(id) => t.album_id = *id,
                PdbLookup::Label(id) => t.label_id = *id,
                PdbLookup::Key(id) => t.key_id = *id,
            }
            ex.add_lookups(&t, l)?;
        }
        PdbOp::AddTrack(t) => ex.add_track(t, l)?,
        PdbOp::UpdateTrack(t) => ex.update_track(t)?,
        PdbOp::AddArtwork { track } => {
            let art: &Artwork = l.artwork.get(&track.artwork_id).ok_or_else(|| {
                Error::Plan(format!("image {} is not in the plan", track.artwork_id))
            })?;
            ex.db.txn(|db| insert_row(db, art))?;
            ex.update_track(track)?;
        }
        PdbOp::AddEntry(e) => {
            ex.add_entry(e)?;
        }
        PdbOp::Reopen => ex.reopen(),
    }
    Ok(())
}

fn write_pdb(plan: &ExportPlan, dev: &DeviceState, rb: &Path, ops: &[PdbOp]) -> Result<()> {
    let existing = dev.pdb.as_ref().map(|p| p.export_bytes.as_slice());
    let bytes = build_pdb(plan.pdb_create.as_ref(), existing, ops, &plan.pdb_lookups)?;
    let path = rb.join("export.pdb");
    write_replace(&path, &bytes)?;
    full_fsync_file(&path)
}

/// Builds exportExt.pdb by running `ops`: the file is created with its DBPROPERTYOPTION row,
/// or continued; each My Tag sync deletes the old tree (and links) and inserts the new one.
pub fn build_ext(
    create: Option<&ExtDb>,
    existing: Option<&[u8]>,
    ops: &[ExtOp],
) -> Result<Vec<u8>> {
    let (mut db, mut tags, mut links): (Db, Vec<RowRef>, Vec<RowRef>) = match (create, existing) {
        (Some(rows), _) => {
            let mut db = Db::create(DbKind::ExportExt);
            db.txn(|db| {
                for _ in 0..DbKind::ExportExt.tables().len() {
                    db.create_table()?;
                }
                for p in &rows.db_property_options {
                    insert_row(db, p)?;
                }
                Ok(())
            })?;
            (db, Vec::new(), Vec::new())
        }
        (None, Some(bytes)) => {
            let pdb = Pdb::parse(bytes.to_vec())?;
            let tags = locate_rows::<MyTag>(&pdb)?
                .into_iter()
                .map(|(r, _)| r)
                .collect();
            let links = locate_rows::<MyTagTrack>(&pdb)?
                .into_iter()
                .map(|(r, _)| r)
                .collect();
            (Db::load(DbKind::ExportExt, bytes)?, tags, links)
        }
        (None, None) => return Err(Error::Plan("no exportExt.pdb to continue".into())),
    };
    for op in ops {
        match op {
            ExtOp::SyncMyTags {
                tags: tree,
                links: new_links,
            } => {
                let old_tags = std::mem::take(&mut tags);
                let old_links = std::mem::take(&mut links);
                if !old_tags.is_empty() || !old_links.is_empty() {
                    db.txn(|db| {
                        for r in &old_links {
                            db.delete(MyTagTrack::TABLE_TYPE, *r)?;
                        }
                        for r in &old_tags {
                            db.delete(MyTag::TABLE_TYPE, *r)?;
                        }
                        Ok(())
                    })?;
                }
                let order = mb_pdb::writer::my_tag_tree_order(tree);
                if !order.is_empty() {
                    tags = db.txn(|db| {
                        order
                            .iter()
                            .map(|t| insert_row(db, t))
                            .collect::<mb_pdb::Result<Vec<_>>>()
                    })?;
                }
                if !new_links.is_empty() {
                    links = db.txn(|db| {
                        new_links
                            .iter()
                            .map(|t| insert_row(db, t))
                            .collect::<mb_pdb::Result<Vec<_>>>()
                    })?;
                }
            }
            ExtOp::AddMyTagLinks(rows) => {
                let refs = db.txn(|db| {
                    rows.iter()
                        .map(|t| insert_row(db, t))
                        .collect::<mb_pdb::Result<Vec<_>>>()
                })?;
                links.extend(refs);
            }
            ExtOp::Reopen => db.reopen(),
        }
    }
    db.close();
    Ok(db.to_bytes())
}

fn write_ext(plan: &ExportPlan, dev: &DeviceState, rb: &Path, ops: &[ExtOp]) -> Result<()> {
    let existing = dev.pdb.as_ref().and_then(|p| p.ext_bytes.as_deref());
    let bytes = build_ext(plan.ext_create.as_ref(), existing, ops)?;
    let path = rb.join("exportExt.pdb");
    write_replace(&path, &bytes)?;
    full_fsync_file(&path)
}

// ---- file helpers ---------------------------------------------------------------------------

fn mkdirs(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))
}

/// `fsync(2)`: pushes the file's data to the device (no drive cache flush).
fn fsync(f: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: `f` owns a valid file descriptor for the duration of the call.
    if unsafe { libc::fsync(f.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// `F_FULLFSYNC` (Rust's `sync_all` on macOS): data on the medium, drive cache flushed.
fn full_fsync_file(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| Error::io(path, e))
}

/// Best-effort `F_FULLFSYNC` of a directory (flushes the drive cache for everything written
/// before; some file systems refuse it on directories).
fn full_fsync_dir(dir: &Path) {
    if let Ok(f) = std::fs::File::open(dir) {
        let _ = f.sync_all();
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        mkdirs(dir)?;
    }
    let mut f = std::fs::File::create(path).map_err(|e| Error::io(path, e))?;
    f.write_all(bytes).map_err(|e| Error::io(path, e))?;
    fsync(&f).map_err(|e| Error::io(path, e))
}

/// The temporary name a replacement of `path` is written under: `.<name>.musicbox` next to it.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.musicbox"))
}

/// Writes `bytes` next to `path`, `fsync`s and renames it into place.
pub(crate) fn write_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    mkdirs(dir)?;
    let tmp = temp_path(path);
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        f.write_all(bytes).map_err(|e| Error::io(&tmp, e))?;
        fsync(&f).map_err(|e| Error::io(&tmp, e))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
}

/// Copies the bytes of `from` to a new file `to` (no extended attributes: `std::fs::copy` would
/// add them, and on FAT each becomes an AppleDouble file), and `fsync`s it.
fn copy_file(from: &Path, to: &Path) -> Result<()> {
    let mut src = std::fs::File::open(from).map_err(|e| Error::io(from, e))?;
    let mut out = std::fs::File::create(to).map_err(|e| Error::io(to, e))?;
    std::io::copy(&mut src, &mut out).map_err(|e| Error::io(to, e))?;
    fsync(&out).map_err(|e| Error::io(to, e))
}

/// `sync_volume_np(3)`: writes every dirty buffer of the volume holding `path` and waits for
/// it; with `full`, also flushes the drive cache (`F_FULLFSYNC`). One call for any number of
/// files. Works on FSKit FAT (checked 2026-10-15: 300 MB flushed in 0.55 s, an `fsync` right
/// after took 1 ms).
fn sync_volume(path: &Path, full: bool) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        unsafe extern "C" {
            fn sync_volume_np(path: *const libc::c_char, flags: libc::c_int) -> libc::c_int;
        }
        const SYNC_VOLUME_FULLSYNC: libc::c_int = 0x01;
        const SYNC_VOLUME_WAIT: libc::c_int = 0x02;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        let flags = SYNC_VOLUME_WAIT | if full { SYNC_VOLUME_FULLSYNC } else { 0 };
        // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
        match unsafe { sync_volume_np(c.as_ptr(), flags) } {
            0 => Ok(()),
            errno => Err(std::io::Error::from_raw_os_error(errno)),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, full);
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// 32 lowercase hex digits (a `juce::Uuid` as rekordbox prints it).
fn random_hex32() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| Error::Device(format!("random source: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}
