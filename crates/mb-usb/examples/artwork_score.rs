//! `cargo run --release -p mb-usb --example artwork_score -- [fixture dir] [options.json] [-v]`
//!
//! Scores `mb_usb::artwork` against rekordbox's own artwork files: for every track of the
//! fixture's master.db with an `ImagePath`, the embedded picture is read from the audio file
//! and the local set (`artwork.jpg`, `_s`, `_m`) is made again; each file is compared with
//! rekordbox's (`rekordbox/share/PIONEER/Artwork/…`) for byte identity, identical headers
//! (tables and frame) and decoded PSNR / largest channel difference. The USB export's
//! `a|b<N>[_m].jpg` files (`rbtest/PIONEER/Artwork`) are matched to the local file they copy
//! and scored against the file we would copy. Defaults: `fixtures/oracle-a`,
//! `fixtures/oracle-b/options.json`.

use mb_masterdb::{LibraryPaths, MasterDb, derive_passphrase};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const VARIANTS: [&str; 3] = ["artwork.jpg", "artwork_s.jpg", "artwork_m.jpg"];

#[derive(Default)]
struct Score {
    files: usize,
    identical: usize,
    same_header: usize,
    psnr: Vec<f64>,
    max_diff: u8,
}

impl Score {
    fn add(&mut self, ours: &[u8], theirs: &[u8]) -> (bool, f64, u8) {
        self.files += 1;
        let same = ours == theirs;
        self.identical += usize::from(same);
        self.same_header += usize::from(header(ours) == header(theirs));
        let (psnr, diff) = compare(ours, theirs);
        self.psnr.push(psnr);
        self.max_diff = self.max_diff.max(diff);
        (same, psnr, diff)
    }

    fn line(&self, name: &str) -> String {
        let mut p = self.psnr.clone();
        p.sort_by(f64::total_cmp);
        let pct = |q: f64| p.get(((p.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(f64::NAN);
        format!(
            "{name:<16} files {:>4}  identical {:>4}  same header {:>4}  PSNR min {:>5.1} median {:>5.1} dB  max |diff| {}",
            self.files,
            self.identical,
            self.same_header,
            pct(0.0),
            pct(0.5),
            self.max_diff
        )
    }
}

/// Everything before the entropy-coded data: APP0, DQT, SOF0, DHT, SOS.
fn header(b: &[u8]) -> &[u8] {
    let mut i = 2;
    while i + 4 <= b.len() {
        let len = usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
        let end = i + 2 + len;
        if b[i + 1] == 0xDA {
            return &b[..end.min(b.len())];
        }
        i = end;
    }
    b
}

/// PSNR (∞ when equal) and the largest channel difference of two decoded JPEGs.
fn compare(a: &[u8], b: &[u8]) -> (f64, u8) {
    let (Ok(a), Ok(b)) = (image::load_from_memory(a), image::load_from_memory(b)) else { return (f64::NAN, 255) };
    let (a, b) = (a.to_rgb8(), b.to_rgb8());
    if a.dimensions() != b.dimensions() {
        return (f64::NAN, 255);
    }
    let mut se = 0f64;
    let mut max = 0u8;
    for (x, y) in a.as_raw().iter().zip(b.as_raw()) {
        let d = x.abs_diff(*y);
        max = max.max(d);
        se += f64::from(d) * f64::from(d);
    }
    let mse = se / a.as_raw().len() as f64;
    (if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }, max)
}

/// What kinds of pictures the run covered (distinct pictures only).
#[derive(Default)]
struct Coverage {
    seen: std::collections::HashSet<[u8; 32]>,
    jpeg: usize,
    png: usize,
    other: usize,
    alpha: usize,
    large: usize,
    non_square: usize,
    odd_margin: usize,
    icc: usize,
    progressive: usize,
}

/// (has an ICC profile, is progressive) from a picture's markers or chunks.
fn picture_flags(b: &[u8]) -> (bool, bool) {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        return (b.windows(4).any(|w| w == b"iCCP"), false);
    }
    let (mut icc, mut progressive, mut i) = (false, false, 2);
    while i + 4 <= b.len() && b[i] == 0xFF {
        let m = b[i + 1];
        let len = usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
        icc |= m == 0xE2 && b[i + 4..].starts_with(b"ICC_PROFILE");
        progressive |= m == 0xC2;
        if m == 0xDA {
            break;
        }
        i += 2 + len;
    }
    (icc, progressive)
}

impl Coverage {
    fn add(&mut self, picture: &[u8]) {
        if !self.seen.insert(sha(picture)) {
            return;
        }
        match picture {
            [0xFF, 0xD8, ..] => self.jpeg += 1,
            [0x89, b'P', b'N', b'G', ..] => self.png += 1,
            _ => self.other += 1,
        }
        let (icc, progressive) = picture_flags(picture);
        self.icc += usize::from(icc);
        self.progressive += usize::from(progressive);
        let Ok(img) = mb_usb::artwork::decode(picture) else { return };
        self.alpha += usize::from(img.data.chunks_exact(4).any(|p| p[3] != 255));
        self.large += usize::from(img.width.max(img.height) > 800);
        if img.width != img.height {
            self.non_square += 1;
            self.odd_margin += usize::from(img.width.abs_diff(img.height) % 2 == 1);
        }
    }
}

fn sha(b: &[u8]) -> [u8; 32] {
    Sha256::digest(b).into()
}

fn jpgs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            jpgs(&p, out);
        } else if p.extension().is_some_and(|x| x == "jpg") && !p.file_name().unwrap().to_string_lossy().starts_with("._") {
            out.push(p);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "-v");
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let fixture = PathBuf::from(pos.first().map_or("fixtures/oracle-a", |s| s.as_str()));
    let options = PathBuf::from(pos.get(1).map_or("fixtures/oracle-b/options.json", |s| s.as_str()));
    let paths = LibraryPaths::from_options_json(&options)?;
    let db = MasterDb::open_read_only(&fixture.join("rekordbox/master.db"), &derive_passphrase(&paths.dp)?)?;
    let rows: Vec<(String, String, String)> = db
        .connection()
        .prepare("SELECT ID, FolderPath, ImagePath FROM djmdContent WHERE rb_local_deleted = 0 AND ImagePath <> '' ORDER BY FolderPath")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let share = fixture.join("rekordbox/share");

    let mut local: [Score; 3] = Default::default();
    // sha of rekordbox's local file → our bytes for the same variant
    let mut ours_by_theirs: HashMap<[u8; 32], Vec<u8>> = HashMap::new();
    let (mut missing_audio, mut no_picture, mut failed) = (0, 0, 0);
    let mut cover = Coverage::default();
    for (id, audio, image_path) in &rows {
        let audio = Path::new(audio);
        if !audio.exists() {
            missing_audio += 1;
            continue;
        }
        let Some(picture) = mb_library::tags::embedded_picture(audio) else {
            no_picture += 1;
            eprintln!("{id}: no embedded picture in {}", audio.display());
            continue;
        };
        cover.add(&picture);
        let set = match mb_usb::artwork::local_set(&picture) {
            Ok(s) => s,
            Err(e) => {
                failed += 1;
                eprintln!("{id}: {e}");
                continue;
            }
        };
        let theirs_dir = share.join(image_path.trim_start_matches('/'));
        for (k, (name, ours)) in VARIANTS.iter().zip([&set.original, &set.small, &set.medium]).enumerate() {
            let Ok(theirs) = std::fs::read(theirs_dir.with_file_name(name)) else { continue };
            let (same, psnr, diff) = local[k].add(ours, &theirs);
            if verbose && !same {
                let dims = image::load_from_memory(&picture).map(|i| (i.width(), i.height())).unwrap_or_default();
                println!("{id} {name}: PSNR {psnr:.2} dB, max |diff| {diff}, picture {}×{}, {}", dims.0, dims.1, audio.display());
            }
            ours_by_theirs.insert(sha(&theirs), ours.clone());
        }
    }

    let mut usb: [Score; 2] = Default::default();
    let mut unmatched = 0;
    let mut device = Vec::new();
    jpgs(&fixture.join("rbtest/PIONEER/Artwork"), &mut device);
    for p in &device {
        let theirs = std::fs::read(p)?;
        let Some(ours) = ours_by_theirs.get(&sha(&theirs)) else {
            unmatched += 1;
            continue;
        };
        let k = usize::from(p.file_stem().unwrap().to_string_lossy().ends_with("_m"));
        usb[k].add(ours, &theirs);
    }

    println!("{} tracks with ImagePath; audio missing {missing_audio}, no picture {no_picture}, failed {failed}", rows.len());
    for (k, name) in VARIANTS.iter().enumerate() {
        println!("{}", local[k].line(&format!("local {}", name.trim_end_matches(".jpg"))));
    }
    println!("{}", usb[0].line("usb a|b<N>"));
    println!("{}", usb[1].line("usb a|b<N>_m"));
    println!("device files not matching a scored local file: {unmatched}");
    println!(
        "pictures: {} distinct; JPEG {} (progressive {}), PNG {}, other {}; ICC profile {}; with transparency {}; larger than 800 px {}; non-square {} (odd margin {})",
        cover.seen.len(),
        cover.jpeg,
        cover.progressive,
        cover.png,
        cover.other,
        cover.icc,
        cover.alpha,
        cover.large,
        cover.non_square,
        cover.odd_margin
    );
    Ok(())
}
