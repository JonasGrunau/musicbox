//! The `.3EX` embedding (rekordbox 7's track embedding for its recommendations):
//! `feature_extractor::embed` @0x10089e85c with rekordbox's ONNX models from the user's install
//! ([`mb_nn::ort`]). The track is read at 16 kHz mono, cut into ~29 s windows with 50 % overlap,
//! and the audio CNN gives 64 features per window; their mean is projected to 2-D (UMAP) and
//! rated for energy and genre. Spec: `docs/re/analysis-embedding.md`.

pub mod msgpack;

use mb_nn::ort::{OnnxModel, OrtSession, OrtTensor};
use std::path::Path;

/// Input rate of the audio CNN.
pub const RATE: u32 = 16000;
/// Window length and hop in samples (`get_features(…, 464000, 512)`: both rounded down to
/// multiples of 256).
const WIN: usize = 463_872;
const HOP: usize = 231_936;
/// Windows per CNN run.
const BATCH: usize = 16;
const N_FEAT: usize = 64;

/// What `embed` computes from the audio.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    /// d4: samples read (`L − 1`) / 16000.
    pub duration: f64,
    /// d5: the CNN features of every window.
    pub windows: Vec<Vec<f32>>,
    /// d6: their mean (without the first and last window when there are three or more).
    pub mean: Vec<f32>,
    /// d7: the normalised 2-D UMAP position.
    pub position: [f32; 2],
    /// d8: energy, clipped to 0…1.
    pub energy: f32,
    /// d9: genre class.
    pub genre: i64,
}

/// The embedding models, loaded once.
pub struct Models {
    cnn: OrtSession,
    norm_before: OrtSession,
    umap: OrtSession,
    norm_after: OrtSession,
    energy: OrtSession,
    genre: OrtSession,
}

impl Models {
    /// `threads` as rekordbox picks it: half the CPUs, at least one.
    pub fn threads() -> usize {
        (std::thread::available_parallelism().map_or(1, |n| n.get()) / 2).max(1)
    }

    fn load() -> Result<Self, String> {
        let t = Self::threads();
        let open = |m| OrtSession::open(m, t).map_err(|e| e.to_string());
        Ok(Self {
            cnn: open(OnnxModel::CnnEmbedding)?,
            norm_before: open(OnnxModel::NormBeforeUmap)?,
            umap: open(OnnxModel::UmapEmbedder)?,
            norm_after: open(OnnxModel::NormAfterUmap)?,
            energy: open(OnnxModel::Energy)?,
            genre: open(OnnxModel::Genre)?,
        })
    }

    pub fn get() -> Result<&'static Models, String> {
        static M: std::sync::OnceLock<Result<Models, String>> = std::sync::OnceLock::new();
        M.get_or_init(Self::load).as_ref().map_err(Clone::clone)
    }

    fn f32s(t: &OrtTensor) -> Result<&[f32], String> {
        match t {
            OrtTensor::F32 { data, .. } => Ok(data),
            OrtTensor::F64 { .. } | OrtTensor::I64 { .. } => Err("f32 output expected".into()),
        }
    }

    /// The CNN over windows of audio (`cnn_run` @0x10089d2b4): its STFT runs inside the model
    /// with frame length 512, step 256 and 256 samples of padding on both sides.
    fn features(&self, rows: &[&[f32]]) -> Result<Vec<Vec<f32>>, String> {
        let mut data = Vec::with_capacity(rows.len() * WIN);
        for r in rows {
            data.extend_from_slice(r);
        }
        let signal = OrtTensor::F32 { dims: vec![rows.len() as i64, WIN as i64], data };
        let i64s = |v: Vec<i64>| OrtTensor::I64 { dims: vec![v.len() as i64], data: v };
        let out = self
            .cnn
            .run(
                &[("signal", &signal), ("frame_length", &i64s(vec![512])), ("frame_step", &i64s(vec![256])), ("pads", &i64s(vec![0, 256, 0, 256]))],
                &["features"],
            )
            .map_err(|e| e.to_string())?;
        Ok(Self::f32s(&out[0])?.chunks_exact(N_FEAT).map(<[f32]>::to_vec).collect())
    }

    fn run1(s: &OrtSession, input: &str, x: &[f32], output: &str) -> Result<Vec<f32>, String> {
        let t = OrtTensor::F32 { dims: vec![1, x.len() as i64], data: x.to_vec() };
        let out = s.run(&[(input, &t)], &[output]).map_err(|e| e.to_string())?;
        Ok(Self::f32s(&out[0])?.to_vec())
    }

    /// `embed`'s model part on 16 kHz mono samples.
    pub fn embed(&self, mono: &[f32]) -> Result<Embedding, String> {
        let duration = mono.len() as f64 / f64::from(RATE);
        // shorter tracks are padded with silence to one window
        let mut audio = mono.to_vec();
        if audio.len() < WIN {
            audio.resize(WIN, 0.0);
        }
        let starts: Vec<usize> = (0..audio.len()).step_by(HOP).filter(|s| s + WIN <= audio.len()).collect();
        let rows: Vec<&[f32]> = starts.iter().map(|&s| &audio[s..s + WIN]).collect();
        let mut windows = Vec::with_capacity(rows.len());
        for batch in rows.chunks(BATCH) {
            windows.extend(self.features(batch)?);
        }
        let mean = mean_rows(if windows.len() >= 3 { &windows[1..windows.len() - 1] } else { &windows });
        let n = Self::run1(&self.norm_before, "input_features", &mean, "normalized_features")?;
        let u = Self::run1(&self.umap, "input_1", &n, "dense_6")?;
        let p = Self::run1(&self.norm_after, "input_features", &u, "normalized_features")?;
        let position = [p[0], p[1]];
        let e = Self::run1(&self.energy, "input_features", &mean, "danceability")?;
        let energy = e[0].clamp(0.0, 1.0);
        let mut g_in = mean.clone();
        g_in.extend_from_slice(&position);
        let t = OrtTensor::F32 { dims: vec![1, g_in.len() as i64], data: g_in };
        let out = self.genre.run(&[("input_features", &t)], &["output_label"]).map_err(|e| e.to_string())?;
        let genre = match &out[0] {
            OrtTensor::I64 { data, .. } => data[0],
            OrtTensor::F32 { .. } | OrtTensor::F64 { .. } => return Err("genre label expected as i64".into()),
        };
        Ok(Embedding { duration, windows, mean, position, energy, genre })
    }
}

/// NumCpp's `mean` over rows: per column, the f32 values summed into an f64 from 0 in order,
/// divided by the row count, then cast to f32.
fn mean_rows(rows: &[Vec<f32>]) -> Vec<f32> {
    let n = rows.len().max(1) as f64;
    (0..N_FEAT)
        .map(|c| {
            let s = rows.iter().fold(0.0f64, |a, r| a + f64::from(r[c]));
            (s / n) as f32
        })
        .collect()
}

/// The `.3EX` record as `notify_embedding` @0x10162c910 writes it: `{"embedding": {…}}` with
/// the keys in rekordbox's order d4 d5 d6 d7 d8 d9 d2 d3 d12 d11 d10.
#[derive(Debug, Clone)]
pub struct Record {
    pub embedding: Embedding,
    /// d2: `djmdContent.ID`.
    pub content_id: String,
    /// d3: MD5 of the audio file, lowercase hex.
    pub md5: String,
    /// d12: share of the track the vocal detector marks as vocal, 0…1.
    pub vocal_share: f64,
    /// d11: the analysed BPM (`bpm × 100 · 0.01`).
    pub bpm: f64,
    /// d10: the analysed key (−1 when key detection failed).
    pub key: i64,
}

impl Record {
    pub fn to_value(&self) -> msgpack::Value {
        use msgpack::Value as V;
        let e = &self.embedding;
        let floats = |v: &[f32]| V::Array(v.iter().map(|&x| V::double(f64::from(x))).collect());
        let fields = vec![
            ("d4".into(), V::double(e.duration)),
            ("d5".into(), V::Array(e.windows.iter().map(|w| floats(w)).collect())),
            ("d6".into(), floats(&e.mean)),
            ("d7".into(), floats(&e.position)),
            ("d8".into(), V::double(f64::from(e.energy))),
            ("d9".into(), V::Int(e.genre)),
            ("d2".into(), V::Str(self.content_id.clone())),
            ("d3".into(), V::Str(self.md5.clone())),
            ("d12".into(), V::double(self.vocal_share)),
            ("d11".into(), V::double(self.bpm)),
            ("d10".into(), V::Int(self.key)),
        ];
        V::Map(vec![("embedding".into(), V::Map(fields))])
    }

    /// The bytes of `ANLZ0000.3EX`.
    pub fn to_3ex(&self) -> Vec<u8> {
        msgpack::encode(&self.to_value())
    }
}

/// The track length in whole seconds as the analysis thread keeps it: `(L − 1) / rate` in
/// integer division at the analysis rate, stored as u16. The embedding runs only for 1…1800 s.
pub fn whole_seconds(total_samples: u64, rate: u32) -> u16 {
    (total_samples.saturating_sub(1) / u64::from(rate.max(1))) as u16
}

pub const MIN_SECONDS: u16 = 1;
pub const MAX_SECONDS: u16 = 1800;

/// One PVDI frame (1024 samples at 22.05 kHz) in µs.
const FRAME_US: f64 = 46439.90929705215;
/// Gaps shorter than this between two vocal parts are bridged, and parts must be longer than
/// this to count (ms).
const MIN_PART_MS: f64 = 6000.0;
/// A part counts only when some frame in it reaches this level.
const MIN_PART_LEVEL: u8 = 3;

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Start,
    Continue,
    Stop,
}

/// d12: the share of the track in vocal parts (`get_percentage_vocals` @0x1008a2308), from the
/// PVDI levels and [`whole_seconds`].
///
/// 1. A mark at every frame where the level (`& 7`) changes, at `trunc(i·46439.909… + 0.5)·0.001`
///    ms (`CBeatAnalyzeThread::analyze` lambda @0x10015a850).
/// 2. The first mark starts a part; then a mark between two non-zero levels continues it, a
///    rise from 0 starts one and a fall to 0 stops it. A stop followed by a start less than
///    6 s later becomes two continues (`get_annotated_vocals` @0x10089fc68).
/// 3. Parts run from a start to the next start; one that does not end in a stop gets one 1 s
///    after its last mark (`get_segments` @0x1008a04c4).
/// 4. Parts longer than 6 s that reach level 3 count (`filter_segments` @0x1008a2140); their
///    lengths in s over the whole seconds, at most 1.
pub fn vocal_share(pvdi: &[u8], seconds: u16) -> f64 {
    let mut marks: Vec<(f64, u8, Option<Mark>)> = Vec::new();
    let mut prev = 0;
    for (i, &b) in pvdi.iter().enumerate() {
        let level = b & 7;
        if level != prev {
            let ms = (i as i32 as f64).mul_add(FRAME_US, 0.5).trunc() * 0.001;
            marks.push((ms, level, None));
            prev = level;
        }
    }
    if marks.is_empty() {
        return 0.0;
    }
    marks[0].2 = Some(Mark::Start);
    for i in 1..marks.len() {
        let (a, b) = (marks[i - 1].1, marks[i].1);
        if a >= 1 && b >= 1 {
            marks[i].2 = Some(Mark::Continue);
        } else if a == 0 && b > 0 {
            marks[i].2 = Some(Mark::Start);
        } else if a > 0 && b == 0 {
            marks[i].2 = Some(Mark::Stop);
        }
    }
    for i in 1..marks.len() {
        if marks[i - 1].2 == Some(Mark::Stop) && marks[i].2 == Some(Mark::Start) && marks[i].0 - marks[i - 1].0 < MIN_PART_MS {
            marks[i].2 = Some(Mark::Continue);
            marks[i - 1].2 = Some(Mark::Continue);
        }
    }
    let mut parts: Vec<Vec<(f64, u8, Option<Mark>)>> = Vec::new();
    for m in marks {
        match parts.last_mut() {
            Some(p) if m.2 != Some(Mark::Start) => p.push(m),
            _ => parts.push(vec![m]),
        }
    }
    let mut total = 0.0;
    for mut p in parts {
        let last = p[p.len() - 1];
        if last.2 != Some(Mark::Stop) {
            p.push((last.0 + 1000.0, 0, Some(Mark::Stop)));
        }
        let max = p.iter().map(|m| m.1).max().unwrap_or(0);
        let duration = p[p.len() - 1].0 - p[0].0;
        if max >= MIN_PART_LEVEL && duration > MIN_PART_MS {
            total += duration;
        }
    }
    (total * 0.001 / f64::from(seconds)).min(1.0)
}

/// MD5 of a file, lowercase hex (d3; rekordbox hashes the whole audio file).
pub fn file_md5(path: &Path) -> std::io::Result<String> {
    use md5::{Digest, Md5};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Md5::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `embed` on the audio file at `path`: [`load_audio`], then [`Models::embed`].
pub fn embed_file(path: &Path) -> Result<Embedding, String> {
    let mono = load_audio(path)?;
    Models::get()?.embed(&mono)
}

/// `load_audio` @0x10089b14c: the track at 16 kHz through rekordbox's converter, samples
/// `0 … L − 2`, stereo as `(R + L)·0.5`.
pub fn load_audio(path: &Path) -> Result<Vec<f32>, String> {
    let mut dec = mb_audio::Decoder::open_at(path, RATE).map_err(|e| e.to_string())?;
    let info = *dec.info();
    let n = info.total_samples.saturating_sub(1) as usize;
    let mut frames = vec![[0f32; 2]; n];
    let got = dec.read_frames(&mut frames);
    frames.truncate(got);
    Ok(if info.channels == 2 { frames.iter().map(|f| (f[1] + f[0]) * 0.5).collect() } else { frames.iter().map(|f| f[0]).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(runs: &[(u8, usize)]) -> Vec<u8> {
        runs.iter().flat_map(|&(l, n)| std::iter::repeat_n(l, n)).collect()
    }

    #[test]
    fn vocal_share_bridges_short_gaps() {
        // vocal at frame 10, a 50-frame gap (2.3 s, bridged), a quieter part to the end: one part
        // from 464.399 ms to 1 s after the last mark at 12074.376 ms
        let v = levels(&[(0, 10), (3, 200), (0, 50), (1, 100)]);
        let want = (12074.376 + 1000.0 - 464.399) * 0.001 / 20.0;
        assert!((vocal_share(&v, 20) - want).abs() < 1e-12);
        // never level 3, or shorter than 6 s: nothing counts
        assert_eq!(vocal_share(&levels(&[(0, 10), (2, 400)]), 20), 0.0);
        assert_eq!(vocal_share(&levels(&[(4, 100), (0, 10)]), 20), 0.0);
        assert_eq!(vocal_share(&[], 20), 0.0);
        // a level that never changes is one mark: its part is the added second
        assert_eq!(vocal_share(&levels(&[(4, 1000)]), 10), 0.0);
        // at most 1 (13.9 s of vocals over 5 s)
        assert_eq!(vocal_share(&levels(&[(4, 300), (0, 1)]), 5), 1.0);
    }
}
