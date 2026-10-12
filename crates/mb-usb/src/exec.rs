//! The executor: runs an [`ExportPlan`] against the device. Spec: `docs/re/export.md` (algorithm
//! summary, § 1.2, § 3, § 4.5, § 5, § 7 "must"), writer checklist.
//!
//! Order on the device:
//! 1. `PIONEER/rekordbox/`, the "export in progress" markers (`sync.temp`,
//!    `sync.devlibplus.temp`, § 4.5), `DeviceLibBackup/rbDevLibBaInfo_<DBID>.json`, the settings
//!    files (§ 5).
//! 2. Per track, in plan order: audio (bytes only: no extended attributes, so no AppleDouble
//!    `._` files on FAT), stale ANLZ sets deleted, the USB ANLZ set, artwork. Each file is
//!    `fsync`ed; a free-space check (≥ 1.5 × the file) precedes each copy.
//! 3. One `F_FULLFSYNC`, then the databases: exportLibrary.db, export.pdb, exportExt.pdb, each
//!    built next to its final name and renamed into place, each `F_FULLFSYNC`ed. Only the
//!    tracks whose files were all written are in them.
//! 4. The markers are deleted; the export log is written for tracks that were not exported.
//!
//! Cancellation or a failed copy stops before the next track; the databases are still written
//! for the tracks done so far, so the stick stays consistent.

use crate::device::{DeviceState, free_bytes};
use crate::log::{LogStrings, entries, write_log};
use crate::naming::pioneer_dir;
use crate::plan::{
    ArtworkSize, DevlibChange, DevlibOp, ExportPlan, ExtOp, FileOp, PdbLookup, PdbOp, TrackResult,
    blank_pdb_track,
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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
        }
    }
    for (path, bytes) in settings_writes(
        opts.settings_dir.as_deref(),
        &pioneer,
        opts.apply_my_settings,
    ) {
        write_replace(&path, &bytes)?;
    }
    // Device Library Plus conversion: `a` artwork copied to `b` (§ 6.4; the database itself is
    // built with the others at the end).
    if let Some(conv) = &plan.devlib_convert {
        crate::convert::copy_artwork(root, conv)?;
    }

    // Tracks.
    let mut report = ExportReport {
        results: steps.iter().map(|s| s.result).collect(),
        ..Default::default()
    };
    ctx.state.phase = Phase::Tracks;
    for (i, step) in steps.iter().enumerate() {
        if cancel.is_cancelled() {
            report.cancelled = true;
            break;
        }
        ctx.state.current = Some(step.title.clone());
        ctx.report();
        match run_files(&mut ctx, &step.files) {
            Ok(()) => {}
            Err(StepError::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(StepError::Failed(result, e)) => {
                report.results[i] = result;
                report.error = Some(e.to_string());
                break;
            }
        }
        report.completed = i + 1;
        ctx.state.tracks_done = i + 1;
    }
    ctx.state.current = None;

    // Databases last.
    ctx.state.phase = Phase::Databases;
    ctx.report();
    full_fsync_dir(root);
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

    ctx.state.phase = Phase::Finish;
    ctx.report();
    if has_lists {
        for m in &markers {
            let _ = std::fs::remove_file(m);
        }
    }
    full_fsync_dir(&rb);
    if let Some(dir) = &opts.log_dir {
        let body = entries(&plan.jobs, &report.results);
        report.log_file =
            write_log(dir, &plan.today, &opts.log_strings, &body).map_err(|e| Error::io(dir, e))?;
    }
    ctx.state.tracks_done = report.completed;
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

enum StepError {
    Cancelled,
    Failed(TrackResult, Error),
}

fn run_files(ctx: &mut Ctx, files: &[FileOp]) -> std::result::Result<(), StepError> {
    for f in files {
        match f {
            FileOp::CopyAudio { from, to, size } => {
                let dest = ctx.fs(to);
                if ctx.check_space {
                    let parent = dest.parent().unwrap_or(ctx.root);
                    let probe = if parent.exists() { parent } else { ctx.root };
                    match free_bytes(probe) {
                        Some(0) => {
                            return Err(StepError::Failed(
                                TrackResult::DeviceGone,
                                Error::Device("the volume reports no space".into()),
                            ));
                        }
                        Some(free) if (free as f64) < 1.5 * (*size as f64) => {
                            return Err(StepError::Failed(
                                TrackResult::NoSpace,
                                Error::Device(format!("{free} bytes free, {size} needed × 1.5")),
                            ));
                        }
                        _ => {}
                    }
                }
                copy_audio(ctx, from, &dest)?;
            }
            FileOp::DeleteAnlz { dat } => {
                let p = ctx.fs(dat);
                for ext in ["DAT", "EXT", "2EX"] {
                    let _ = std::fs::remove_file(p.with_extension(ext));
                }
            }
            FileOp::WriteAnlz {
                local_dat,
                parts,
                dat,
                ppth,
                cues,
            } => {
                let dest = ctx.fs(dat);
                write_anlz(local_dat, *parts, &dest, ppth, cues)
                    .map_err(|e| StepError::Failed(TrackResult::AnalysisFailed, e))?;
            }
            FileOp::Artwork { from, size, to } => {
                let dest = ctx.fs(to);
                let bytes = artwork_bytes(from, *size)
                    .map_err(|e| StepError::Failed(TrackResult::CopyFailed, e))?;
                write_replace(&dest, &bytes)
                    .map_err(|e| StepError::Failed(TrackResult::CopyFailed, e))?;
            }
        }
    }
    Ok(())
}

/// Copies the audio bytes only (no extended attributes or resource forks, which macOS would
/// store as `._` AppleDouble files on FAT), 1 MiB at a time, checking for cancellation.
fn copy_audio(ctx: &mut Ctx, from: &Path, dest: &Path) -> std::result::Result<(), StepError> {
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
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|e| failed(Error::io(dest, e)))?;
            ctx.state.bytes_done += n as u64;
            ctx.report();
        }
        fsync(&out).map_err(|e| failed(Error::io(dest, e)))
    })();
    if result.is_err() {
        drop(out);
        let _ = std::fs::remove_file(dest);
    }
    result
}

/// The USB ANLZ set from the local one (`ExportAnalysisData*`, `docs/re/anlz.md` § Local vs
/// USB): PPTH = device path, PSSI/PVDI scrambled, the cue lists from master.db (`.DAT` and
/// `.EXT`), everything else unchanged. `.3EX` is never exported.
fn write_anlz(local_dat: &Path, parts: [bool; 3], dest_dat: &Path, ppth: &str, cues: &crate::cues::DeviceCues) -> Result<()> {
    for (ext, present) in ["DAT", "EXT", "2EX"].iter().zip(parts) {
        if !present {
            continue;
        }
        let src = local_dat.with_extension(ext);
        let bytes = std::fs::read(&src).map_err(|e| Error::io(&src, e))?;
        let file = mb_anlz::AnlzFile::parse(&bytes).map_err(|source| Error::Anlz {
            path: src.clone(),
            source,
        })?;
        let mut usb = mb_anlz::usb::to_usb(&file, ppth);
        match *ext {
            "DAT" => crate::cues::apply_dat(&mut usb, cues),
            "EXT" => crate::cues::apply_ext(&mut usb, cues),
            _ => {}
        }
        let usb = usb.to_bytes();
        write_replace(&dest_dat.with_extension(ext), &usb)?;
    }
    Ok(())
}

/// The bytes of one device artwork file: the local variant as is, or — when the variant is
/// missing (`create_small_arts_if_not_exists`) — generated from the base image: centred on an
/// opaque black square, scaled, baseline JPEG quality 85 (§ 3). Generated files are not
/// byte-identical to rekordbox's JUCE encoder.
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
    let img = image::open(&art.base).map_err(|e| fail(e.to_string()))?;
    let (w, h) = (img.width(), img.height());
    let side = w.max(h);
    let mut canvas = image::RgbImage::from_pixel(side, side, image::Rgb([0, 0, 0]));
    image::imageops::overlay(
        &mut canvas,
        &img.to_rgb8(),
        i64::from((side - w) / 2),
        i64::from((side - h) / 2),
    );
    let px = size.pixels();
    let scaled = image::imageops::resize(&canvas, px, px, image::imageops::FilterType::Lanczos3);
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&scaled)
        .map_err(|e| fail(e.to_string()))?;
    Ok(out)
}

// ---- databases ------------------------------------------------------------------------------

fn write_devlib(plan: &ExportPlan, rb: &Path, completed: Option<usize>) -> Result<()> {
    let target = rb.join("exportLibrary.db");
    let tmp = rb.join(".exportLibrary.db.musicbox");
    remove_db_files(&tmp);
    let mut writer = match (&plan.devlib_create, &plan.devlib_convert) {
        (Some(create), _) => DevLibWriter::create(
            &tmp,
            &CreateOptions {
                created_date: Some(create.created_date.clone()),
                menus: create.menus.clone(),
                ..CreateOptions::default()
            },
        )?,
        // Device Library Plus conversion of an old stick (§ 6.4), then the export's operations.
        (None, Some(conv)) => {
            let w = DevLibWriter::create_for_conversion(
                &tmp,
                &CreateOptions {
                    created_date: Some(conv.today.clone()),
                    menus: conv.menus.clone(),
                    ..CreateOptions::default()
                },
            )?;
            if let Err(e) = conv.write(&w) {
                drop(w);
                remove_db_files(&tmp);
                return Err(e);
            }
            w
        }
        (None, None) => {
            std::fs::copy(&target, &tmp).map_err(|e| Error::io(&target, e))?;
            DevLibWriter::open(&tmp)?
        }
    };
    writer.set_today(&plan.today);
    let result = (|| -> Result<()> {
        writer.begin()?;
        for op in plan.devlib_ops(completed) {
            apply_devlib(&writer, &op, plan.hidden)?;
        }
        writer.commit()?;
        Ok(())
    })();
    if let Err(e) = result {
        drop(writer);
        remove_db_files(&tmp);
        return Err(e);
    }
    writer.close()?;
    full_fsync_file(&tmp)?;
    std::fs::rename(&tmp, &target).map_err(|e| Error::io(&target, e))?;
    full_fsync_file(&target)
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

/// Writes `bytes` next to `path`, `fsync`s and renames it into place.
pub(crate) fn write_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    mkdirs(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.musicbox"));
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        f.write_all(bytes).map_err(|e| Error::io(&tmp, e))?;
        fsync(&f).map_err(|e| Error::io(&tmp, e))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
}

/// 32 lowercase hex digits (a `juce::Uuid` as rekordbox prints it).
fn random_hex32() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| Error::Device(format!("random source: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}
