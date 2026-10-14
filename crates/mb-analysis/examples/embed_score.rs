//! The `.3EX` embedding against rekordbox's (the 14 files of `fixtures/sandbox-initial`). Usage:
//!   cargo run --release -p mb-analysis --example embed_score [filter]
//! Per track: every stored value compared bit for bit after the JSON step, and the file bytes.
//! d10 (key), d11 (BPM) and d12 (vocal share) come from our own analysis of the track.

#[path = "../tests/common/mod.rs"]
mod common;

use mb_analysis::embed::{self, msgpack::Value, Record};

fn floats(v: &Value) -> Vec<f64> {
    match v {
        Value::Array(a) => a.iter().flat_map(floats).collect(),
        v => v.as_f64().into_iter().collect(),
    }
}

fn cmp(ours: &[f64], rb: &[f64]) -> (usize, usize, f64) {
    let n = ours.len().min(rb.len());
    let exact = (0..n).filter(|&i| ours[i].to_bits() == rb[i].to_bits()).count();
    let max = (0..n).map(|i| (ours[i] - rb[i]).abs()).fold(0f64, f64::max);
    (exact, ours.len().max(rb.len()), max)
}

fn main() -> anyhow::Result<()> {
    let filter = std::env::args().nth(1);
    let root = common::fixtures().join("sandbox-initial");
    let Some(tracks) = common::oracle_tracks("sandbox-initial") else {
        anyhow::bail!("fixtures/sandbox-initial missing");
    };
    let models = embed::Models::get().map_err(anyhow::Error::msg)?;
    let mut files = Vec::new();
    let mut stack = vec![root.join("rekordbox/share/PIONEER/USBANLZ")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "ANLZ0000.3EX") {
                files.push(p);
            }
        }
    }
    files.sort();
    let (mut total, mut same_values, mut same_bytes) = (0, 0, 0);
    for f in &files {
        let bytes = std::fs::read(f)?;
        let rb = embed::msgpack::decode(&bytes).map_err(anyhow::Error::msg)?;
        let e = rb.get("embedding").ok_or_else(|| anyhow::anyhow!("no embedding"))?;
        let id = e.get("d2").and_then(Value::as_str).unwrap_or_default();
        let Some(t) = tracks.iter().find(|t| t.id == id) else {
            println!("{id}: not in master.db");
            continue;
        };
        if filter.as_ref().is_some_and(|q| !t.path.to_string_lossy().contains(q.as_str())) {
            continue;
        }
        total += 1;
        let name = t.path.file_name().unwrap_or_default().to_string_lossy();
        let t0 = std::time::Instant::now();
        let mono = embed::load_audio(&t.path).map_err(anyhow::Error::msg)?;
        let ours = models.embed(&mono).map_err(anyhow::Error::msg)?;
        let a = mb_analysis::analyze(&t.path, mb_analysis::Options { vocal: true, ..Default::default() })?;
        let info = *mb_audio::open(&t.path)?.info();
        let secs = embed::whole_seconds(info.total_samples, info.sample_rate);
        let rec = Record {
            embedding: ours,
            content_id: id.into(),
            md5: embed::file_md5(&t.path)?,
            vocal_share: a.vocal.as_deref().map_or(0.0, |v| embed::vocal_share(v, secs)),
            // the BPM the track holds after the run: the library's (the sampler loops carry
            // preset grids that the beat pass does not replace)
            bpm: t.bpm.unwrap_or(0) as f64 * 0.01,
            key: a.key.map_or(-1, i64::from),
        };
        let mine = rec.to_value();
        let me = mine.get("embedding").unwrap();
        let mut all = true;
        let mut line = format!("{name:24} {:5.1}s", t0.elapsed().as_secs_f64());
        for k in ["d4", "d5", "d6", "d7", "d8"] {
            let (x, n, max) = cmp(&floats(me.get(k).unwrap()), &floats(e.get(k).unwrap()));
            all &= x == n;
            line += &format!(" | {k} {x}/{n}");
            if x != n {
                line += &format!(" (max {max:.2e})");
            }
        }
        for k in ["d9", "d3", "d12", "d11", "d10"] {
            let ok = me.get(k) == e.get(k);
            all &= ok;
            line += &format!(" | {k} {}", if ok { "ok" } else { "DIFF" });
        }
        let ours_bpm = a.beats.as_ref().map_or(0, |g| i64::from(g.bpm));
        if ours_bpm != t.bpm.unwrap_or(0) {
            line += &format!(" | our grid {ours_bpm} vs library {:?}", t.bpm);
        }
        for k in ["d9", "d12", "d11", "d10"] {
            if me.get(k) != e.get(k) {
                line += &format!(" ({k} {:?} vs {:?})", me.get(k), e.get(k));
            }
        }
        let bytes_ok = rec.to_3ex() == bytes;
        same_values += usize::from(all);
        same_bytes += usize::from(bytes_ok);
        println!("{line} | bytes {}", if bytes_ok { "same" } else { "differ" });
    }
    println!("{same_values}/{total} identical values, {same_bytes}/{total} identical files");
    Ok(())
}
