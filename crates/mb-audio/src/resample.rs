//! rekordbox's sample-rate converter, ported for bit-exact output (spec: `docs/re/decode.md` § SRC).
//!
//! * [`Kaiser`]: `rbxfrm::f2dsp::c_src<Float2>` — a polyphase FIR with L = out/50 phases of 29 taps,
//!   designed by `initIdxAndIrForKaiser` @0x10288d270 (windowed sinc, Kaiser window via a 22-term
//!   Bessel series), run by `convert_pull` @0x10288b848 with `convolute_fine` @0x10288d01c.
//! * [`SampleRateConverter`]: `rbxfrm::SampleRateConverter` @0x10284b490, which pulls blocks of
//!   2048 output samples and reads exactly the input `calcInputNum` asks for.
//!
//! Floating-point details follow the arm64 code: the coefficient design uses fused multiply-adds
//! where the binary does (`fnmsub`, `fmadd`) and the factorials are multiplied in the same
//! vectorised order; the convolution accumulates with `fmla` (fused) from both ends inwards.

use crate::reader::{Float2, Reader, ReaderInfo};
use crate::{Error, Result};

const TAPS: usize = 29;
/// History buffer length in Float2 (`c_src` +0x20 … +0x900).
const BUF: usize = 284;

/// Cut-off (relative to max(in, out)/50) and Kaiser β per rate pair
/// (`SamplingFunctionSRC<Float2>::init` @0x10288b2c0).
pub fn design_params(input: u32, output: u32) -> (f64, f64) {
    match (input, output) {
        (44100, 48000) => (f64::from_bits(0x3fe1_47ae_147a_e148), 8.0), // 0.54
        (48000, 44100) => (0.52, 8.0),
        (44100, 96000) | (44100, 88200) => (0.58, 10.0),
        (48000, 96000) => (0.45, 10.0),
        (96000, 44100) | (88200, 44100) => (f64::from_bits(0x3fe1_9999_9999_999a), 6.0), // 0.55
        (96000, 48000) => (0.47, 10.0),
        (44100, 176400) => (0.48, 9.0),
        (44100, 192000) => (0.48, 9.0),
        _ => (0.45, 12.0),
    }
}

unsafe extern "C" {
    fn pow(x: f64, y: f64) -> f64;
    fn sin(x: f64) -> f64;
}

/// The system libm `pow`, as the binary calls it for every term (black-boxed so the compiler
/// cannot substitute `x·x` or constants for small exponents).
fn libm_pow(x: f64, y: f64) -> f64 {
    // SAFETY: pure libm function.
    unsafe { pow(std::hint::black_box(x), std::hint::black_box(y)) }
}

fn libm_sin(x: f64) -> f64 {
    // SAFETY: pure libm function.
    unsafe { sin(std::hint::black_box(x)) }
}

/// Factorial of `j` in the exact multiplication order of the binary (scalar below 9!, then
/// eight 2-lane accumulators combined as ((a8·(a6·(a4·a2))) · (a9·(a7·(a5·a3))), then scalar).
fn factorial(j: u32) -> f64 {
    if j < 2 {
        return 1.0;
    }
    let jm1 = j - 1;
    if jm1 < 8 {
        let mut d = 1.0f64;
        for m in 2..=j {
            d *= f64::from(m);
        }
        return d;
    }
    let mut acc = [[1.0f64; 2]; 4];
    let mut base = 2u32;
    for _ in 0..(jm1 & !7) / 8 {
        for (k, a) in acc.iter_mut().enumerate() {
            for (lane, v) in a.iter_mut().enumerate() {
                *v *= f64::from(base + 2 * k as u32 + lane as u32);
            }
        }
        base += 8;
    }
    let lane = |l: usize| acc[3][l] * (acc[2][l] * (acc[1][l] * acc[0][l]));
    let mut d = lane(0) * lane(1);
    if jm1 != jm1 & !7 {
        for m in ((jm1 & !7) | 2)..=j {
            d *= f64::from(m);
        }
    }
    d
}

/// Windowed-sinc prototype of `n` taps (`initIdxAndIrForKaiser`, first loop).
fn prototype(n: usize, fc: f64, beta: f64) -> Vec<f64> {
    let half = n as i64 / 2;
    let two_pi_fc = fc * std::f64::consts::TAU;
    let two_fc = fc + fc;
    let inv = 1.0 / (n as f64 - 1.0);
    let step = inv + inv;
    let half_beta = beta * 0.5;
    let mut h = Vec::with_capacity(n);
    let mut facts = [0.0f64; 22];
    for (j, f) in facts.iter_mut().enumerate() {
        let x = factorial(j as u32);
        *f = 1.0 / (x * x);
    }
    for k in 0..n.max(1) {
        let m = k as i64 - half;
        let sinc = if m == 0 {
            two_fc
        } else {
            let mf = m as f64;
            libm_sin(two_pi_fc * mf) / (mf * std::f64::consts::PI)
        };
        let x = k as f64 * step;
        let t = x + -1.0;
        let arg = (1.0 - x.mul_add(t, -t)).sqrt() * half_beta;
        let (mut num, mut den) = (0.0f64, 0.0f64);
        let mut jj = 0.0f64;
        for f in facts {
            let e = jj + jj;
            num = f.mul_add(libm_pow(arg, e), num);
            den = f.mul_add(libm_pow(half_beta, e), den);
            jj += 1.0;
        }
        h.push((num * sinc) / den);
    }
    h
}

/// `c_src<Float2>`: the polyphase resampler state.
#[derive(Clone)]
pub struct Kaiser {
    l: i32,
    m: i32,
    /// First input index per phase (`+0x10`).
    idx: Vec<i32>,
    /// 29 coefficients per phase (`+0x18`; both channels share them).
    coefs: Vec<[f32; TAPS]>,
    buf: Box<[Float2; BUF]>,
    p: i32,
    base: i32,
    w: i32,
}

impl Kaiser {
    pub fn new(input: u32, output: u32) -> Result<Self> {
        let (cutoff, beta) = design_params(input, output);
        Self::with_params(input, output, cutoff, beta)
    }

    /// `c_src::init_kaiser` @0x10288b570: both rates must be multiples of 50 (output ≤ 200 kHz).
    pub fn with_params(input: u32, output: u32, cutoff: f64, beta: f64) -> Result<Self> {
        if !input.is_multiple_of(50) || !output.is_multiple_of(50) || !(8000..=200_000).contains(&input) || !(8000..=200_000).contains(&output) {
            return Err(Error::Format(format!("no rekordbox SRC for {input} → {output} Hz")));
        }
        let l = (output / 50) as i32;
        let m = (input / 50) as i32;
        let fc = cutoff / f64::from(l.max(m));
        let n = l as usize * TAPS;
        let h = prototype(n, fc, beta);
        let lf = f64::from(l);
        let mut idx = Vec::with_capacity(l as usize);
        let mut coefs = Vec::with_capacity(l as usize);
        for p in 0..l {
            let first = (m * p) / l;
            idx.push(first);
            let mut c = [0.0f32; TAPS];
            for (t, v) in c.iter_mut().enumerate() {
                let i = l * (t as i32 + first) + l + !(m * p);
                // (h · L) · 1.0: the normalisation flag is off in every caller.
                *v = (h[i as usize] * lf * 1.0) as f32;
            }
            coefs.push(c);
        }
        let mut k = Self { l, m, idx, coefs, buf: Box::new([[0.0; 2]; BUF]), p: 0, base: 0, w: 0 };
        k.reset();
        Ok(k)
    }

    /// `resetConvert`: 28 zeros of history, phase 0.
    pub fn reset(&mut self) {
        self.buf.fill([0.0; 2]);
        self.w = 28;
        self.p = 0;
        self.base = 0;
    }

    #[inline]
    fn ready(&self) -> bool {
        (self.idx[self.p as usize] - self.base) + TAPS as i32 <= self.w
    }

    #[inline]
    fn advance(&mut self) {
        self.p += 1;
        if self.p >= self.l {
            self.p = 0;
            self.base -= self.m;
        }
    }

    /// `convolute_fine`: fused accumulation from both ends towards the middle tap.
    #[inline]
    fn convolve(&self) -> Float2 {
        let start = (self.idx[self.p as usize] - self.base) as usize;
        let x = &self.buf[start..start + TAPS];
        let c = &self.coefs[self.p as usize];
        let mut acc = [0.0f32; 2];
        for i in 0..TAPS / 2 {
            let j = TAPS - 1 - i;
            for ch in 0..2 {
                acc[ch] = x[i][ch].mul_add(c[i], acc[ch]);
                acc[ch] = x[j][ch].mul_add(c[j], acc[ch]);
            }
        }
        let mid = TAPS / 2;
        for ch in 0..2 {
            acc[ch] = x[mid][ch].mul_add(c[mid], acc[ch]);
        }
        acc
    }

    fn produce(&mut self, out: &mut [Float2], o: &mut usize) {
        out[*o] = self.convolve();
        *o += 1;
        self.advance();
    }

    /// `convert_pull` (`convertSpecifyingOutputNum`): produces `out.len()` samples, consuming input
    /// as needed; returns the number of inputs consumed.
    pub fn convert_pull(&mut self, input: &[Float2], out: &mut [Float2]) -> usize {
        let mut n_out = out.len() as i64;
        let mut o = 0usize;
        let mut consumed = 0usize;
        if self.ready() && n_out > 0 {
            loop {
                self.produce(out, &mut o);
                if !self.ready() || n_out < 2 {
                    break;
                }
                n_out -= 1;
            }
            n_out -= 1;
        }
        if n_out < 1 {
            return consumed;
        }
        loop {
            loop {
                let Some(&x) = input.get(consumed) else { return consumed };
                self.buf[self.w as usize] = x;
                let old = self.w;
                self.w += 1;
                if old >= 0x11b {
                    self.buf.copy_within(255..284, 0);
                    self.w = 29;
                    self.base += 255;
                }
                if self.ready() {
                    break;
                }
                consumed += 1;
            }
            let mut u = n_out;
            loop {
                self.produce(out, &mut o);
                n_out = u - 1;
                let more = u > 1;
                u = n_out;
                if !(self.ready() && more) {
                    break;
                }
            }
            consumed += 1;
            if n_out <= 0 {
                break;
            }
        }
        consumed
    }

    /// `SamplingFunctionSRC::calcInputNum` @0x10288bafc: inputs needed for `n` more outputs.
    pub fn calc_input_num(&self, n: i64) -> i64 {
        let (l, m, idx) = (self.l, self.m, &self.idx);
        let (mut p, mut base, mut w) = (self.p, self.base, self.w);
        let ready = |p: i32, base: i32, w: i32| (idx[p as usize] - base) + 29 <= w;
        let mut n = n;
        if n > 0 && ready(p, base, w) {
            let mut u = n;
            loop {
                p += 1;
                if p >= l {
                    p = 0;
                    base -= m;
                }
                n = u - 1;
                let more = u > 1;
                u = n;
                if !(ready(p, base, w) && more) {
                    break;
                }
            }
        }
        if n <= 0 {
            return 0;
        }
        let mut count = 0;
        loop {
            let (base2, w2) = if w < 0x11b { (base, w + 1) } else { (base + 0xff, 0x1d) };
            base = base2;
            if ready(p, base, w2) {
                let mut u = n;
                loop {
                    p += 1;
                    if p >= l {
                        p = 0;
                        base -= m;
                    }
                    n = u - 1;
                    let more = u != 1;
                    u = n;
                    if !(ready(p, base, w2) && more) {
                        break;
                    }
                }
            }
            count += 1;
            w = w2;
            if n <= 0 {
                break;
            }
        }
        count
    }

    /// `SamplingFunctionSRC::calcOutputNum` @0x10288bed4: outputs `n` more inputs yield.
    pub fn calc_output_num(&self, n: i64) -> i64 {
        let (l, m, idx) = (self.l, self.m, &self.idx);
        let (mut p, mut base, mut w) = (self.p, self.base, self.w);
        let mut count = 0i64;
        let step = |p: &mut i32, base: &mut i32| {
            *p += 1;
            if *p >= l {
                *p = 0;
                *base -= m;
            }
        };
        if n >= 1 {
            let mut n = n;
            'outer: loop {
                while w > 0x11a {
                    if (idx[p as usize] - base) + 0x1c <= w {
                        loop {
                            step(&mut p, &mut base);
                            count += 1;
                            if (idx[p as usize] - base) + 0x1c > w {
                                break;
                            }
                        }
                    }
                    base += 0x100;
                    w = 0x1c;
                    let done = n < 2;
                    n -= 1;
                    if done {
                        break 'outer;
                    }
                }
                w += 1;
                let more = n > 1;
                n -= 1;
                if !more {
                    break;
                }
            }
        }
        if w < (idx[p as usize] - base) + 0x1d {
            return count;
        }
        loop {
            step(&mut p, &mut base);
            count += 1;
            if (idx[p as usize] - base) + 0x1d > w {
                break;
            }
        }
        count
    }
}

/// `rbxfrm::SampleRateConverter` over a reader: a [`Reader`] at the target rate.
pub struct SampleRateConverter<R: Reader> {
    reader: R,
    src: Kaiser,
    in_rate: u32,
    out_rate: u32,
    in_len: i64,
    out_len: i64,
    /// Next input position (`+0x40`) and output position (`+0x44`).
    in_pos: i64,
    out_pos: i64,
    in_buf: Vec<Float2>,
}

/// Output samples per processing block (`+0x4c`).
const BLOCK: usize = 0x800;

impl<R: Reader> SampleRateConverter<R> {
    /// `SampleRateConverter::Init` @0x10284b76c.
    pub fn new(reader: R, out_rate: u32) -> Result<Self> {
        let info = reader.info();
        let src = Kaiser::new(info.sample_rate, out_rate)?;
        let in_len = i64::from(info.length);
        // updateTrackLength: outputs a fresh converter yields from the whole input.
        let out_len = src.calc_output_num(in_len);
        Ok(Self { reader, src, in_rate: info.sample_rate, out_rate, in_len, out_len, in_pos: 0, out_pos: 0, in_buf: Vec::new() })
    }

    pub fn inner(&self) -> &R {
        &self.reader
    }

    /// `processFileRead` @0x10284c578: input samples, silence outside the track.
    fn read_input(&mut self, pos: i64, n: usize) {
        self.in_buf.resize(n, [0.0; 2]);
        let mut done = 0usize;
        while done < n {
            let k = (n - done).min(0x200);
            let (a, b) = (pos + done as i64, pos + (done + k) as i64);
            let slice = &mut self.in_buf[done..done + k];
            if b <= 0 || a >= self.in_len {
                slice.fill([0.0; 2]);
            } else {
                self.reader.read(a, slice);
                // Positions at or past the input length are zero (+0x38 clamp).
                if b > self.in_len {
                    let keep = (self.in_len - a) as usize;
                    slice[keep..].fill([0.0; 2]);
                }
            }
            done += k;
        }
    }

    /// `prepareSRCProcessing` @0x10284bd70: re-primes the filter for a random-access read.
    /// Returns the samples already written to `out`.
    fn prepare(&mut self, pos: i64, out: &mut [Float2]) -> usize {
        let pos = if pos < 0 || pos > self.out_len { 0 } else { pos };
        self.src.reset();
        self.out_pos = pos;
        let gcd = gcd(self.in_rate, self.out_rate).max(1);
        let mut out_at = pos;
        if self.in_rate < self.out_rate {
            let k = i64::from((self.out_rate / gcd).max(1));
            let q = pos / k;
            if pos != q * k {
                out_at = k + k * q;
                self.out_pos = out_at;
            }
        }
        let in_at = self.src.calc_input_num(out_at);
        self.in_pos = in_at;
        let k = i64::from((self.in_rate / gcd).max(1));
        let prime_from = ((in_at - 30) / k) * k;
        let mut n_in = in_at - prime_from;
        let mut n_out = self.src.calc_output_num(n_in);
        if n_out > BLOCK as i64 {
            n_out = BLOCK as i64;
            n_in = self.src.calc_input_num(n_out);
        }
        let prime_from = in_at - n_in;
        let ahead = (self.out_pos - pos) as usize;
        self.read_input(prime_from, n_in as usize);
        let mut prime_out = vec![[0.0f32; 2]; n_out as usize];
        let input = std::mem::take(&mut self.in_buf);
        self.src.convert_pull(&input, &mut prime_out);
        self.in_buf = input;
        let ahead = ahead.min(out.len()).min(prime_out.len());
        out[..ahead].copy_from_slice(&prime_out[prime_out.len() - ahead..]);
        ahead
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

impl<R: Reader> Reader for SampleRateConverter<R> {
    fn info(&self) -> ReaderInfo {
        let i = self.reader.info();
        ReaderInfo { sample_rate: self.out_rate, length: self.out_len.clamp(0, i64::from(u32::MAX)) as u32, ..i }
    }

    /// The next read re-primes (`prepareSRCProcessing`), wherever it starts.
    fn reposition(&mut self) {
        self.out_pos = i64::MIN;
    }

    /// `SampleRateConverter::fileRead` @0x10284bab8.
    fn read(&mut self, pos: i64, out: &mut [Float2]) {
        let mut out = out;
        if self.out_pos != pos {
            let done = self.prepare(pos, out);
            if done > 0 {
                out = &mut out[done..];
            }
        }
        while !out.is_empty() {
            let n = out.len().min(BLOCK);
            let n_in = self.src.calc_input_num(n as i64) as usize;
            self.read_input(self.in_pos, n_in);
            let input = std::mem::take(&mut self.in_buf);
            let used = self.src.convert_pull(&input, &mut out[..n]);
            debug_assert_eq!(used, n_in);
            self.in_buf = input;
            self.in_pos += n_in as i64;
            self.out_pos += n as i64;
            out = &mut out[n..];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factorials_are_close_to_exact() {
        let mut exact = 1.0f64;
        for j in 0..22u32 {
            if j > 1 {
                exact *= f64::from(j);
            }
            let f = factorial(j);
            assert!(((f - exact) / exact).abs() < 1e-15, "{j}: {f} vs {exact}");
        }
        // Up to 18! every partial product is exact, so the vectorised order cannot matter yet.
        assert_eq!(factorial(18), 6_402_373_705_728_000.0);
    }

    /// An in-memory stream.
    struct Mem {
        rate: u32,
        data: Vec<Float2>,
    }

    impl Reader for Mem {
        fn info(&self) -> ReaderInfo {
            ReaderInfo { sample_rate: self.rate, channels: 2, bits: 32, length: self.data.len() as u32 }
        }
        fn read(&mut self, pos: i64, out: &mut [Float2]) {
            for (i, o) in out.iter_mut().enumerate() {
                let p = pos + i as i64;
                *o = if p < 0 || p >= self.data.len() as i64 { [0.0; 2] } else { self.data[p as usize] };
            }
        }
    }

    /// 1000 samples of silence, then a sawtooth pair.
    fn signal(rate: u32, n: usize) -> Mem {
        Mem { rate, data: (0..n).map(|i| if i < 1000 { [0.0; 2] } else { [((i % 37) as f32 - 18.0) / 20.0, ((i % 23) as f32 - 11.0) / 12.0] }).collect() }
    }

    fn read_all(c: &mut SampleRateConverter<Mem>, n: usize) -> Vec<Float2> {
        let mut out = vec![[0.0; 2]; n];
        c.read(0, &mut out);
        out
    }

    /// A read from the start after a rewind re-primes; the result does not depend on what was
    /// read before. For 22.05 → 44.1 kHz it equals the fresh stream, for 96 → 48 kHz it does not
    /// (`analysis-phrase.md` § 1.4).
    #[test]
    fn rewound_reads_reprime() {
        for (i, o, same) in [(22050, 44100, true), (96000, 48000, false)] {
            let mut c = SampleRateConverter::new(signal(i, 40_000), o).unwrap();
            let n = c.info().length as usize;
            let fresh = read_all(&mut c, n);
            c.reposition();
            let again = read_all(&mut c, n);
            let mut d = SampleRateConverter::new(signal(i, 40_000), o).unwrap();
            d.reposition();
            assert_eq!(read_all(&mut d, n), again, "{i}->{o}: the re-primed stream is reproducible");
            assert_eq!(fresh == again, same, "{i}->{o}");
            // a plain sequential read from the start does not re-prime
            let mut e = SampleRateConverter::new(signal(i, 40_000), o).unwrap();
            assert_eq!(read_all(&mut e, n), fresh);
        }
    }

    #[test]
    fn rejects_rates_rekordbox_cannot_convert() {
        assert!(Kaiser::new(11025, 44100).is_err());
        assert!(Kaiser::new(22050, 44100).is_ok());
    }

    /// Lengths and consumption follow calcOutputNum / calcInputNum for every rate pair the
    /// analysis uses.
    #[test]
    fn pull_consumes_what_calc_input_num_predicts() {
        for (i, o) in [(22050, 44100), (96000, 48000), (32000, 48000), (88200, 44100), (192000, 48000), (24000, 48000)] {
            let mut k = Kaiser::new(i, o).unwrap();
            let input: Vec<Float2> = (0..40_000).map(|n| [((n % 97) as f32) / 97.0, -((n % 89) as f32) / 89.0]).collect();
            let mut at = 0usize;
            let mut produced = 0i64;
            for block in [1usize, 7, 2048, 333, 4096, 1] {
                let need = k.calc_input_num(block as i64) as usize;
                let mut out = vec![[0.0; 2]; block];
                let used = k.convert_pull(&input[at..at + need], &mut out);
                assert_eq!(used, need, "{i}->{o} block {block}");
                at += used;
                produced += block as i64;
            }
            // Producing `produced` outputs from a fresh converter needs `at` inputs.
            let fresh = Kaiser::new(i, o).unwrap();
            assert_eq!(fresh.calc_input_num(produced), at as i64, "{i}->{o}");
            assert!(fresh.calc_output_num(at as i64) >= produced);
        }
    }

    /// Output does not depend on how the stream is cut into blocks.
    #[test]
    fn output_is_block_invariant() {
        let input: Vec<Float2> = (0..30_000).map(|n| [((n * 7 % 101) as f32 - 50.0) / 50.0, ((n % 13) as f32) / 13.0]).collect();
        let run = |blocks: &[usize]| {
            let mut k = Kaiser::new(22050, 44100).unwrap();
            let (mut at, mut out) = (0usize, Vec::new());
            for &b in blocks.iter().cycle().take(200) {
                let need = k.calc_input_num(b as i64) as usize;
                if at + need > input.len() {
                    break;
                }
                let mut o = vec![[0.0; 2]; b];
                at += k.convert_pull(&input[at..at + need], &mut o);
                out.extend(o);
            }
            out
        };
        let a = run(&[2048]);
        let b = run(&[1, 513, 77, 2048]);
        let n = a.len().min(b.len());
        assert!(n > 10_000);
        assert_eq!(a[..n], b[..n]);
    }

    /// 2× upsampling of a fresh converter: output n sits 14.5 input samples (29 outputs) late,
    /// the delay seen on the 22.05 kHz probes.
    #[test]
    fn upsampling_delay() {
        let mut k = Kaiser::new(22050, 44100).unwrap();
        let mut input = vec![[0.0f32; 2]; 400];
        input[100] = [1.0, 1.0];
        let need = k.calc_input_num(600) as usize;
        let mut out = vec![[0.0; 2]; 600];
        k.convert_pull(&input[..need], &mut out);
        let peak = (0..out.len()).max_by(|&a, &b| out[a][0].total_cmp(&out[b][0])).unwrap();
        assert_eq!(peak, 2 * 100 + 29);
    }
}
