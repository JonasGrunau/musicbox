//! `Discriminator::inference` @0x100b812d0: the normalised features cut into windows of 360
//! frames (hop 180), rekordbox's network (`vd_param.pb`: `InputBatch` f32 [windows, 29, 360],
//! `LengthBatch` i32 [windows, 1] → `Softmax` [windows, 360, 2]), the vocal probability averaged
//! over the overlapping windows, and the PVDI byte (`VocalDetector::applyEndProcess`).
//! Spec: `analysis-vocal.md` § 5.

use mb_nn::{Feed, Session, Tensor};

const WIN: usize = 360;
const HOP: usize = 180;
const N_FEAT: usize = 29;

/// The per-frame vocal probability: windows from every 180th frame, `⌈frames/180⌉` of them.
pub fn probabilities(model: &Session, feats: &[f64]) -> mb_nn::Result<Vec<f32>> {
    let frames = feats.len() / N_FEAT;
    // fcvtps: rounded up, so the last window may be short
    let windows = (frames as f64 * f64::from_bits(0x3f76_c16c_16c1_6c17)).ceil().max(0.0) as usize;
    let mut input = vec![0f32; windows * N_FEAT * WIN];
    let mut lens = vec![0i32; windows];
    for s in 0..windows {
        let len = if s * HOP + WIN <= frames { WIN } else { frames - s * HOP };
        lens[s] = len as i32;
        for f in 0..N_FEAT {
            for t in 0..len {
                input[(s * N_FEAT + f) * WIN + t] = feats[(s * HOP + t) * N_FEAT + f] as f32;
            }
        }
    }
    let mut prob = vec![0f32; frames];
    let mut count = vec![0u8; frames];
    if windows > 0 {
        let x = Tensor { dims: vec![windows as i64, N_FEAT as i64, WIN as i64], data: input };
        let ldims = [windows as i64, 1];
        let out = model.run_feeds(
            &[("InputBatch", Feed::F32(&x)), ("LengthBatch", Feed::I32 { dims: &ldims, data: &lens })],
            &["Softmax"],
        )?;
        let sm = &out[0].data;
        for s in 0..windows {
            for t in 0..lens[s] as usize {
                prob[s * HOP + t] += sm[(s * WIN + t) * 2 + 1];
                count[s * HOP + t] = count[s * HOP + t].wrapping_add(1);
            }
        }
    }
    for (p, &c) in prob.iter_mut().zip(&count) {
        *p /= f32::from(c);
    }
    Ok(prob)
}

/// The PVDI value of one frame: the probability in steps of 0.2 (`> 0.2` → 1 … `> 0.8` → 4,
/// compared in f32; NaN → 0).
pub fn level(p: f32) -> u8 {
    let mut v = u8::from(p > f32::from_bits(0x3e4c_cccd));
    if p > f32::from_bits(0x3ecc_cccd) {
        v = 2;
    }
    if p > f32::from_bits(0x3f19_999a) {
        v = 3;
    }
    if p > f32::from_bits(0x3f4c_cccd) {
        v = 4;
    }
    v
}
