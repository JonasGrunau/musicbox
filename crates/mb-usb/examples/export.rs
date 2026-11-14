//! `cargo run -p mb-usb --example export -- <library_dir> <device_root> <playlist>... [options]`
//!
//! Exports playlists from a library directory (`options.json` + `rekordbox/master.db` +
//! `rekordbox/share`, as in `fixtures/`) to a device root (a stick, or any directory for
//! development) the way rekordbox 7.2.19 does. A playlist is given by name or `Folder/Name`
//! path; a folder is exported with its contents.
//!
//! Options:
//! - `--settings-dir <dir>`: local settings folder (MYSETTING.DAT, …, djprofile.nxs); default:
//!   none (rekordbox's defaults, no djprofile.nxs).
//! - `--log-dir <dir>`: where to write the export log; default: no log.
//! - `--date <YYYY-MM-DD>`: the date for a new stick's databases; default: today.
//! - `--dry-run`: print the plan only.
//! - `--stop-after <n>`: cancel when the track after the first `n` starts (a shorter run of a
//!   big playlist; the databases then hold the tracks done).
//! - `--tiny-audio`: copy 1 KiB stand-ins instead of the audio (same files and folders; for
//!   timing the executor's per-file work on a small disk image).
//!
//! The library is never opened in place (master.db is copied to a temp dir). Never point the
//! device root at a reference stick.

use mb_usb::source::LibraryDir;
use mb_usb::{
    CancelToken, DeviceState, ExportOptions, PlanOptions, SelectionItem, SourceLibrary, execute,
    plan_export,
};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut positional = Vec::new();
    let mut opts = ExportOptions::default();
    let mut plan_opts = PlanOptions::default();
    let mut dry_run = false;
    let mut tiny_audio = false;
    let mut stop_after: Option<usize> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--settings-dir" => opts.settings_dir = args.next().map(PathBuf::from),
            "--log-dir" => opts.log_dir = args.next().map(PathBuf::from),
            "--date" => plan_opts.today = args.next().unwrap_or_default(),
            "--dry-run" => dry_run = true,
            "--tiny-audio" => tiny_audio = true,
            "--stop-after" => stop_after = args.next().and_then(|n| n.parse().ok()),
            _ => positional.push(a),
        }
    }
    if positional.len() < 3 {
        anyhow::bail!(
            "usage: export <library_dir|live> <device_root> <playlist>... [--settings-dir D] \
             [--log-dir D] [--date YYYY-MM-DD] [--dry-run]"
        );
    }
    // `live` = the library rekordbox itself uses (paths from rekordboxAgent's options.json).
    let library = if positional[0] == "live" {
        let options = mb_masterdb::LibraryPaths::default_options_json().expect("HOME");
        let paths = mb_masterdb::LibraryPaths::from_options_json(&options)?;
        let pass = mb_masterdb::derive_passphrase(&paths.dp)?;
        if opts.settings_dir.is_none() {
            opts.settings_dir = Some(paths.settings_root.clone());
        }
        LibraryDir::open_paths(&paths.db_path, &pass, paths.analysis_data_root, paths.settings_root)?
    } else {
        LibraryDir::open(&PathBuf::from(&positional[0]))?
    };
    let root = PathBuf::from(&positional[1]);
    std::fs::create_dir_all(&root)?;

    // Resolve the playlist names against the library.
    let all = SourceLibrary::load(library.connection(), &library.analysis_root, &[])?;
    let selection = positional[2..]
        .iter()
        .map(|name| Ok(SelectionItem::Playlist(all.find_playlist(name)?.id.clone())))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let t0 = std::time::Instant::now();
    let source = SourceLibrary::load(library.connection(), &library.analysis_root, &selection)?;
    let t1 = std::time::Instant::now();
    let device = DeviceState::load(&root, None)?;
    let t2 = std::time::Instant::now();
    println!(
        "device {} ({}), {}",
        root.display(),
        device.filesystem.as_deref().unwrap_or("?"),
        if device.pdb.is_some() {
            "existing export"
        } else {
            "new export"
        }
    );
    if let Some(t) = opts.settings_dir.as_deref().and_then(mb_usb::settings::hot_cue_color_type) {
        plan_opts.hot_cue_color_type = t;
    }
    let mut plan = plan_export(&source, &device, &selection, &plan_opts)?;
    println!(
        "load: source {:.2} s, device {:.2} s, plan {:.2} s",
        (t1 - t0).as_secs_f64(),
        (t2 - t1).as_secs_f64(),
        t2.elapsed().as_secs_f64()
    );
    if tiny_audio {
        let stand_in = std::env::temp_dir().join("mb-export-tiny-audio");
        std::fs::write(&stand_in, [0u8; 1024])?;
        for step in plan.jobs.iter_mut().flat_map(|j| j.steps.iter_mut()) {
            for f in &mut step.files {
                if let mb_usb::plan::FileOp::CopyAudio { from, size, .. } = f {
                    *from = stand_in.clone();
                    *size = 1024;
                }
            }
        }
    }
    for job in &plan.jobs {
        let mut counts = std::collections::BTreeMap::<u32, usize>::new();
        for s in &job.steps {
            *counts.entry(s.result.code()).or_default() += 1;
        }
        println!(
            "job {:?}: {} tracks, export.pdb list {}, exportLibrary.db list {}, results {:?}",
            job.name,
            job.steps.len(),
            job.pdb_list,
            job.devlib_list,
            counts
        );
    }
    println!(
        "{:.1} MB of audio to copy",
        plan.bytes_to_copy() as f64 / 1e6
    );
    if dry_run {
        for s in plan.steps() {
            println!(
                "  [{}] {} -> {}",
                s.result.code(),
                s.title,
                s.device_path.as_deref().unwrap_or("-")
            );
        }
        return Ok(());
    }
    let cancel = CancelToken::new();
    let mut last = std::time::Instant::now();
    // Wall time per phase, printed at the end (for comparing executor changes).
    let mut phases: Vec<(mb_usb::Phase, std::time::Instant)> = Vec::new();
    let mut started: Vec<String> = Vec::new();
    let stopper = cancel.clone();
    let report = execute(&plan, &device, &opts, &cancel, &mut |p| {
        if let (Some(n), Some(title)) = (stop_after, &p.current)
            && started.last() != Some(title)
        {
            started.push(title.clone());
            if started.len() > n {
                stopper.cancel();
            }
        }
        if phases.last().is_none_or(|(ph, _)| *ph != p.phase) {
            phases.push((p.phase, std::time::Instant::now()));
        }
        if last.elapsed().as_millis() > 500 {
            last = std::time::Instant::now();
            eprintln!(
                "{:?} {}/{} tracks, {:.0}/{:.0} MB {}",
                p.phase,
                p.tracks_done,
                p.tracks_total,
                p.bytes_done as f64 / 1e6,
                p.bytes_total as f64 / 1e6,
                p.current.as_deref().unwrap_or("")
            );
        }
    })?;
    let end = std::time::Instant::now();
    let timing: Vec<String> = phases
        .iter()
        .enumerate()
        .map(|(i, (ph, at))| {
            let next = phases.get(i + 1).map_or(end, |(_, t)| *t);
            format!("{ph:?} {:.2} s", (next - *at).as_secs_f64())
        })
        .collect();
    println!("timing: {}", timing.join(", "));
    println!(
        "done: {} of {} tracks exported, cancelled {}, error {:?}",
        report.exported(),
        report.results.len(),
        report.cancelled,
        report.error
    );
    if let Some(log) = &report.log_file {
        println!("export log: {}", log.display());
    }
    Ok(())
}
