//! Deterministic mono synthesis at 44.1 kHz.

use std::f64::consts::TAU;

pub const SR: u32 = 44_100;

pub struct Synth {
    buf: Vec<f64>,
    rng: u64,
}

impl Synth {
    pub fn new(seconds: f64) -> Self {
        Self { buf: vec![0.0; (seconds * SR as f64).round() as usize], rng: 0x9E37_79B9_7F4A_7C15 }
    }

    pub fn seconds(&self) -> f64 {
        self.buf.len() as f64 / SR as f64
    }

    pub fn into_samples(self) -> Vec<f32> {
        let peak = self.buf.iter().fold(0.0f64, |m, s| m.max(s.abs()));
        let scale = if peak > 0.99 { 0.99 / peak } else { 1.0 };
        self.buf.into_iter().map(|s| (s * scale) as f32).collect()
    }

    fn idx(t: f64) -> usize {
        (t * SR as f64).round() as usize
    }

    fn white(&mut self) -> f64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let v = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }

    /// Kick on every beat (downbeats +3 dB), closed hat on every offbeat. Returns the time of the
    /// first beat at or after `end` so a following segment can continue on the grid.
    pub fn drums(&mut self, start: f64, end: f64, bpm: f64, gain: f64) -> f64 {
        let gain = if gain == 0.0 { 1.0 } else { gain };
        let period = 60.0 / bpm;
        let mut beat = 0u64;
        loop {
            let t = start + beat as f64 * period;
            if t >= end - 1e-9 {
                return t;
            }
            let accent = if beat % 4 == 0 { 1.41 } else { 1.0 };
            self.kick(t, 0.6 * accent * gain);
            self.hat(t + period / 2.0, 0.15 * gain);
            beat += 1;
        }
    }

    fn kick(&mut self, t: f64, amp: f64) {
        let start = Self::idx(t);
        let len = Self::idx(0.25);
        let mut phase = 0.0;
        for i in 0..len {
            let Some(s) = self.buf.get_mut(start + i) else { break };
            let x = i as f64 / SR as f64;
            let freq = 50.0 + 100.0 * (-x * 40.0).exp();
            phase += TAU * freq / SR as f64;
            *s += amp * phase.sin() * (-x * 18.0).exp();
        }
    }

    fn hat(&mut self, t: f64, amp: f64) {
        let start = Self::idx(t);
        let len = Self::idx(0.04);
        let mut prev = 0.0;
        for i in 0..len {
            let n = self.white();
            let Some(s) = self.buf.get_mut(start + i) else { break };
            let x = i as f64 / SR as f64;
            *s += amp * (n - prev) * (-x * 90.0).exp();
            prev = n;
        }
    }

    /// Triad (root, third, fifth) as band-limited saw pads, MIDI note numbers.
    pub fn chord(&mut self, t: f64, dur: f64, root: u8, major: bool) {
        let notes = [root, root + if major { 4 } else { 3 }, root + 7, root + 12];
        let start = Self::idx(t);
        let len = Self::idx(dur);
        for note in notes {
            let freq = 440.0 * 2f64.powf((note as f64 - 69.0) / 12.0);
            for i in 0..len {
                let Some(s) = self.buf.get_mut(start + i) else { break };
                let x = i as f64 / SR as f64;
                let env = (x / 0.02).min(1.0) * ((dur - x) / 0.05).clamp(0.0, 1.0);
                let mut v = 0.0;
                for h in 1..=8 {
                    if freq * h as f64 > 18_000.0 {
                        break;
                    }
                    v += (TAU * freq * h as f64 * x).sin() / h as f64;
                }
                *s += 0.08 * env * v;
            }
        }
    }

    pub fn tone(&mut self, t0: f64, t1: f64, freq: f64, amp: f64) {
        for i in Self::idx(t0)..Self::idx(t1).min(self.buf.len()) {
            let x = i as f64 / SR as f64;
            self.buf[i] += amp * (TAU * freq * x).sin();
        }
    }

    pub fn sweep(&mut self, t0: f64, t1: f64, f0: f64, f1: f64, amp: f64) {
        let dur = t1 - t0;
        let k = (f1 / f0).ln();
        for i in Self::idx(t0)..Self::idx(t1).min(self.buf.len()) {
            let x = i as f64 / SR as f64 - t0;
            let phase = TAU * f0 * dur / k * ((x / dur * k).exp() - 1.0);
            self.buf[i] += amp * phase.sin();
        }
    }

    pub fn noise(&mut self, t0: f64, t1: f64, amp: f64, pink: bool) {
        // Paul Kellet's economy pink filter.
        let (mut b0, mut b1, mut b2) = (0.0, 0.0, 0.0);
        for i in Self::idx(t0)..Self::idx(t1).min(self.buf.len()) {
            let w = self.white();
            let v = if pink {
                b0 = 0.99765 * b0 + w * 0.0990460;
                b1 = 0.96300 * b1 + w * 0.2965164;
                b2 = 0.57000 * b2 + w * 1.0526913;
                (b0 + b1 + b2 + w * 0.1848) * 0.2
            } else {
                w
            };
            self.buf[i] += amp * v;
        }
    }
}
