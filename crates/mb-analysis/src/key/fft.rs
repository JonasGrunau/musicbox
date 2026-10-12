//! BALib's FFT helpers used by the key analysis (`analysis-key.md` § 3): `make_sintbl`
//! @0x10118656c, `make_bitrev` @0x10118673c, `fft_less_normalize` @0x10118678c, `hanning`
//! @0x10113f634, `calc_mag` @0x1011869c4 and `calc_phase_sincos` @0x101186aac, with the
//! binary's fused multiply-adds.

/// `n + n/4` sine values (the cosine is read a quarter period later).
pub fn make_sintbl(n: usize) -> Vec<f32> {
    let (n2, n4, n8) = (n / 2, n / 4, n / 8);
    let mut t = vec![0f32; n + n4];
    let ts = (std::f64::consts::PI / n as f64).sin();
    t[n4] = 1.0;
    t[0] = 0.0;
    if n >= 16 {
        let s0 = ts as f32;
        let sq = s0 * s0;
        let four = sq * 4.0;
        let mut dc = sq + sq;
        let mut ds = ((2.0 - f64::from(dc)) * f64::from(dc)).sqrt() as f32;
        let (mut c, mut s) = (1f32, 0f32);
        for i in 1..n8.max(2) {
            c -= dc;
            dc = c.mul_add(four, dc);
            s += ds;
            ds = (-s).mul_add(four, ds);
            t[i] = s;
            t[n4 - i] = c;
        }
    }
    if n >= 8 {
        t[n8] = f32::from_bits(0x3f3504f3);
    }
    if n >= 4 {
        for i in 0..n4 {
            t[n2 - i] = t[i];
        }
    }
    for i in 0..n2 + n4 {
        t[i + n2] = -t[i];
    }
    t
}

/// Bit-reversed indices, built by the binary's increment loop.
pub fn make_bitrev(n: usize) -> Vec<usize> {
    let mut b = vec![0usize; n.max(1)];
    let n = n as i32;
    let mut r: i32 = 0;
    for slot in b.iter_mut().skip(1) {
        let mut h = n;
        let mut cur;
        loop {
            cur = r;
            h /= 2;
            r = cur - h;
            if h > cur {
                break;
            }
        }
        r = h + cur;
        *slot = r as usize;
    }
    b
}

/// Unnormalised radix-2 FFT in place; `inverse` for rekordbox's negative `n`.
pub fn fft(re: &mut [f32], im: &mut [f32], inverse: bool, tbl: &[f32], bitrev: &[usize]) {
    let n = bitrev.len();
    for i in 0..n {
        let j = bitrev[i];
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let n4 = n / 4;
    let mut k = 1;
    while k < n {
        let h = 2 * k;
        let d = n / h;
        for j in 0..k {
            let c = tbl[j * d + n4];
            let s = tbl[j * d];
            let mut i = j;
            while i < n {
                let (r1, i1) = (re[i + k], im[i + k]);
                let (tr, ti) = if inverse {
                    // fnmsub: re1·c − (s·im1); fmadd: im1·c + (s·re1)
                    (r1.mul_add(c, -(s * i1)), i1.mul_add(c, s * r1))
                } else {
                    // fmadd: re1·c + (im1·s); fnmsub: im1·c − (re1·s)
                    (r1.mul_add(c, i1 * s), i1.mul_add(c, -(r1 * s)))
                };
                re[i + k] = re[i] - tr;
                re[i] += tr;
                im[i + k] = im[i] - ti;
                im[i] += ti;
                i += h;
            }
        }
        k = h;
    }
}

/// `w[i−1] = 0.5 − cos(2π·i/(n+1))·0.5` (fused); the first `n & !3` points take the angle as
/// `i·(2π/(n+1))`, the rest as `(i·2π)·(1/(n+1))`, like the vectorised and scalar loops.
pub fn hanning(n: usize, w: &mut [f32]) {
    let np1 = (n + 1) as f64;
    let vec_end = if n >= 4 { n & !3 } else { 0 };
    let k = std::f64::consts::TAU / np1;
    let inv = 1.0 / np1;
    for i in 1..=n {
        let x = if i <= vec_end {
            i as f64 * k
        } else {
            i as f64 * std::f64::consts::TAU * inv
        };
        w[i - 1] = (-x.cos()).mul_add(0.5, 0.5) as f32;
    }
}

/// `sqrt(im² + re²)` with the multiply-add fused.
pub fn calc_mag(out: &mut [f32], re: &[f32], im: &[f32]) {
    for ((o, &r), &i) in out.iter_mut().zip(re).zip(im) {
        *o = i.mul_add(i, r * r).sqrt();
    }
}

/// `(sin, cos)` of the phase, computed without trigonometry; bins with `|re| ≤ 2⁻²³` get
/// `sin = 0, cos = ±1`.
pub fn calc_phase_sincos(sin: &mut [f32], cos: &mut [f32], re: &[f32], im: &[f32]) {
    const EPS: f32 = 1.1920929e-07;
    for k in 0..sin.len() {
        let r = re[k];
        let (mut t, mut d) = (0f32, 1f32);
        // fcmp r, eps; fccmp r, −eps, #8, le; b.ge skip
        let small = (-EPS..=EPS).contains(&r);
        if !small {
            t = im[k] / r;
            d = t.mul_add(t, 1.0).sqrt();
        }
        let neg = r < 0.0;
        sin[k] = if neg { -t } else { t } / d;
        cos[k] = if neg { -1.0 } else { 1.0 } / d;
    }
}
