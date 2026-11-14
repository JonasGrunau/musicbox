//! Timing of [`DevLibWriter`] on a large synthetic export: `tracks` new tracks with realistic
//! lookup reuse (shared artists, albums, genres, labels, keys), an image for every other track,
//! then one playlist with `entries` entries — all in one transaction, as
//! `mb_usb::exec::write_devlib` runs an export.
//!
//! ```text
//! cargo run --release -p mb-devlib --example bench_write -- [tracks] [entries] [--dir <dir>]
//!     [--keep <db>] [--compare <db>]
//! ```
//! `--keep` copies the finished database to `<db>`; `--compare` diffs the finished database
//! against an earlier `--keep` copy with [`mb_devlib::diff::compare`] (rows, schema, settings).

use mb_devlib::{CreateOptions, DevLibDb, DevLibWriter, ImageOwner, NewTrack};
use std::path::PathBuf;
use std::time::Instant;

/// xorshift64*: deterministic, so every run builds the same rows.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    /// Skewed pick in `0..n`: low indexes come up far more often (popular artists, genres).
    fn skewed(&mut self, n: u64) -> u64 {
        let a = self.below(n);
        let b = self.below(n);
        a.min(b)
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

const KEYS: [&str; 24] = [
    "1A", "2A", "3A", "4A", "5A", "6A", "7A", "8A", "9A", "10A", "11A", "12A", "1B", "2B", "3B",
    "4B", "5B", "6B", "7B", "8B", "9B", "10B", "11B", "12B",
];

fn track(rng: &mut Rng, i: usize) -> NewTrack {
    let artist = format!("Artist {}", rng.skewed(4000));
    let opt = |rng: &mut Rng, percent, prefix: &str, n| {
        if rng.chance(percent) {
            format!("{prefix} {}", rng.skewed(n))
        } else {
            String::new()
        }
    };
    let remixer = opt(rng, 25, "Artist", 4000);
    let original_artist = opt(rng, 10, "Artist", 4000);
    let composer = opt(rng, 30, "Composer", 1500);
    let lyricist = opt(rng, 10, "Composer", 1500);
    let album_artist = opt(rng, 40, "Artist", 4000);
    // Album names repeat across artists ("Greatest Hits" by several artists).
    let album = opt(rng, 85, "Album", 2500);
    let genre = opt(rng, 90, "Genre", 60);
    let label = opt(rng, 70, "Label", 800);
    let key = if rng.chance(90) {
        KEYS[rng.below(24) as usize].to_string()
    } else {
        String::new()
    };
    let title = format!("Title {i}");
    NewTrack {
        path: format!("/Contents/{artist}/{album}/{title}.mp3"),
        title,
        artist,
        remixer,
        original_artist,
        composer,
        lyricist,
        album,
        album_artist,
        compilation: u8::from(rng.chance(5)),
        genre,
        label,
        key,
        bpm_x100: 9000 + rng.below(9000) as i64,
        length: 120 + rng.below(400) as i64,
        track_no: rng.below(20) as i64,
        release_year: 1970 + rng.below(56) as i64,
        rating: rng.below(6) as i64,
        color_id: rng.below(9) as i64,
        comment: if rng.chance(30) {
            format!("comment {}", rng.below(1000))
        } else {
            String::new()
        },
        date_created: "2026-10-11".into(),
        date_added: "2026-10-11".into(),
        file_size: 5_000_000 + rng.below(10_000_000),
        file_type: 1,
        bitrate: 320,
        bit_depth: 16,
        sample_rate: 44100,
        master_db_id: 7,
        master_content_id: 100 + i as u32,
        analysis_path: format!("/PIONEER/USBANLZ/P{:03X}/{:08X}/ANLZ0000.DAT", i % 4096, i),
        analysed_bits: 41,
        content_link: 788_224,
        ..NewTrack::default()
    }
}

fn main() -> anyhow::Result<()> {
    let mut tracks = 10_000usize;
    let mut entries = 5_000usize;
    let mut dir = std::env::temp_dir();
    let mut keep: Option<PathBuf> = None;
    let mut compare: Option<PathBuf> = None;
    let mut positional = 0;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dir" => dir = args.next().expect("--dir <dir>").into(),
            "--keep" => keep = Some(args.next().expect("--keep <db>").into()),
            "--compare" => compare = Some(args.next().expect("--compare <db>").into()),
            n => {
                let n: usize = n.parse()?;
                if positional == 0 {
                    tracks = n;
                } else {
                    entries = n;
                }
                positional += 1;
            }
        }
    }
    let work = dir.join(format!("mb-devlib-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let path = work.join("exportLibrary.db");

    let total = Instant::now();
    let t = Instant::now();
    let w = DevLibWriter::create(
        &path,
        &CreateOptions {
            created_date: Some("2026-10-11".into()),
            ..CreateOptions::default()
        },
    )?;
    let create_ms = t.elapsed().as_secs_f64() * 1e3;

    w.begin()?;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let t = Instant::now();
    let mut ids = Vec::with_capacity(tracks);
    let mut lap = Instant::now();
    for i in 0..tracks {
        let id = w.add_track(&track(&mut rng, i))?;
        if i % 2 == 0 {
            w.add_image(ImageOwner::Content(id), false)?;
        }
        ids.push(id);
        if (i + 1) % 2_000 == 0 {
            println!(
                "  tracks {:>6}: {:>8.1} ms for the last 2000",
                i + 1,
                lap.elapsed().as_secs_f64() * 1e3
            );
            lap = Instant::now();
        }
    }
    let tracks_ms = t.elapsed().as_secs_f64() * 1e3;

    let t = Instant::now();
    let list = w.create_playlist("bench", 0, false, Some(0))?;
    for i in 0..entries {
        w.append_playlist_entry(list, ids[(i * 7) % ids.len().max(1)])?;
    }
    let entries_ms = t.elapsed().as_secs_f64() * 1e3;

    let t = Instant::now();
    w.commit()?;
    w.close()?;
    let close_ms = t.elapsed().as_secs_f64() * 1e3;
    let total_ms = total.elapsed().as_secs_f64() * 1e3;

    println!("tracks {tracks}, playlist entries {entries}");
    println!("  create            {create_ms:>10.1} ms");
    println!(
        "  add_track+image   {tracks_ms:>10.1} ms ({:.3} ms/track)",
        tracks_ms / tracks.max(1) as f64
    );
    println!(
        "  playlist entries  {entries_ms:>10.1} ms ({:.4} ms/entry)",
        entries_ms / entries.max(1) as f64
    );
    println!("  commit+close      {close_ms:>10.1} ms");
    println!("  total             {total_ms:>10.1} ms");

    if let Some(keep) = &keep {
        std::fs::copy(&path, keep)?;
        println!("kept {}", keep.display());
    }
    if let Some(other) = &compare {
        let ours = DevLibDb::open_read_only(&path)?;
        let theirs_copy = work.join("compare.db");
        std::fs::copy(other, &theirs_copy)?;
        let theirs = DevLibDb::open_read_only(&theirs_copy)?;
        let cmp = mb_devlib::diff::compare(ours.connection(), theirs.connection())?;
        print!("{cmp}");
        println!(
            "content identical to {}: {}",
            other.display(),
            if cmp.content_identical() { "yes" } else { "NO" }
        );
    }
    std::fs::remove_dir_all(&work)?;
    Ok(())
}
