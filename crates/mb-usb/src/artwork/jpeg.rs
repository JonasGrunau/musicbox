//! A baseline JPEG encoder whose output matches IJG libjpeg 6b byte for byte for the one
//! configuration rekordbox uses (JUCE's `JPEGImageFormat::writeImageToStream` @0x102551520,
//! `docs/re/ops/analyse-track.md` § Artwork):
//!
//! - 8-bit RGB in, YCbCr 4:2:0 out (luma 2×2, both chroma 1×1), one interleaved scan, no
//!   restart markers;
//! - the Annex K tables at quality 85 (`jpeg_set_linear_quality` with scale 30 %, forced
//!   baseline);
//! - the floating-point AA&N forward DCT (`JDCT_FLOAT`), with the quantisation rounding done
//!   as one fused multiply-add, as the shipped arm64 code does (`fmla`);
//! - optimised Huffman tables (`optimize_coding`), built with the Annex K.2 procedure;
//! - a JFIF 1.01 APP0 with a 1:1 pixel aspect ratio and no thumbnail.
//!
//! The edge handling follows libjpeg: samples past the right and bottom edges repeat the last
//! column and row; whole blocks past the edge that only fill an MCU ("dummy" blocks) are zero
//! with the DC of the block before them.

/// The Annex K luminance table, in natural (row-major) order.
const LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, //
    12, 12, 14, 19, 26, 58, 60, 55, //
    14, 13, 16, 24, 40, 57, 69, 56, //
    14, 17, 22, 29, 51, 87, 80, 62, //
    18, 22, 37, 56, 68, 109, 103, 77, //
    24, 35, 55, 64, 81, 104, 113, 92, //
    49, 64, 78, 87, 103, 121, 120, 101, //
    72, 92, 95, 98, 112, 100, 103, 99,
];

/// The Annex K chrominance table, in natural order.
const CHROMA: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, //
    18, 21, 26, 66, 99, 99, 99, 99, //
    24, 26, 56, 99, 99, 99, 99, 99, //
    47, 66, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99,
];

/// Zigzag position → natural index.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, //
    12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, //
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, //
    58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// JUCE's default quality 0.85 → libjpeg quality 85 → table scale 200 − 2·85 = 30 %.
const SCALE_PERCENT: u32 = 30;

/// AA&N scale factors: 1 and cos(kπ/16)·√2 for k = 1..7, as libjpeg spells them.
const AAN: [f64; 8] = [1.0, 1.387039845, 1.306562965, 1.175875602, 1.0, 0.785694958, 0.541196100, 0.275899379];

type Block = [i16; 64];

/// The quantisation table for `base` at the scale, natural order (`jpeg_add_quant_table`).
fn quant_table(base: &[u16; 64]) -> [u16; 64] {
    base.map(|b| ((u32::from(b) * SCALE_PERCENT + 50) / 100).clamp(1, 255) as u16)
}

/// Reciprocal divisors of the float quantiser (`start_pass_fdctmgr`, `JDCT_FLOAT`).
fn divisors(q: &[u16; 64]) -> [f32; 64] {
    std::array::from_fn(|i| (1.0 / (f64::from(q[i]) * AAN[i / 8] * AAN[i % 8] * 8.0)) as f32)
}

/// libjpeg's `rgb_ycc_convert`: fixed point with 16 fraction bits; Cb/Cr round with 0.5 − ε.
fn rgb_to_ycc(rgb: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    const ONE_HALF: i32 = 1 << 15;
    let fix = |x: f64| (x * 65536.0 + 0.5) as i32;
    let (ry, gy, by) = (fix(0.29900), fix(0.58700), fix(0.11400));
    let (rcb, gcb, bcb) = (-fix(0.16874), -fix(0.33126), fix(0.50000));
    let (gcr, bcr) = (-fix(0.41869), -fix(0.08131));
    let offset = (128 << 16) + ONE_HALF - 1;
    let n = rgb.len() / 3;
    let (mut y, mut cb, mut cr) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    for p in rgb.chunks_exact(3) {
        let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]));
        y.push(((ry * r + gy * g + by * b + ONE_HALF) >> 16) as u8);
        cb.push(((rcb * r + gcb * g + bcb * b + offset) >> 16) as u8);
        // R→Cr uses the B→Cb entries (both FIX(0.5)).
        cr.push(((bcb * r + gcr * g + bcr * b + offset) >> 16) as u8);
    }
    (y, cb, cr)
}

/// The rotation constants of `jpeg_fdct_float`, as libjpeg writes them, rounded to `f32`.
#[allow(clippy::excessive_precision, clippy::approx_constant)]
mod c {
    pub const C4: f32 = 0.707106781;
    pub const C6: f32 = 0.382683433;
    pub const C2_MINUS_C6: f32 = 0.541196100;
    pub const C2_PLUS_C6: f32 = 1.306562965;
}

/// `jpeg_fdct_float`, in place, row pass then column pass; plain single-precision arithmetic.
fn fdct_float(d: &mut [f32; 64]) {
    use c::*;
    fn pass(d: &mut [f32; 64], base: usize, step: usize) {
        let at = |k: usize| base + k * step;
        let tmp0 = d[at(0)] + d[at(7)];
        let tmp7 = d[at(0)] - d[at(7)];
        let tmp1 = d[at(1)] + d[at(6)];
        let tmp6 = d[at(1)] - d[at(6)];
        let tmp2 = d[at(2)] + d[at(5)];
        let tmp5 = d[at(2)] - d[at(5)];
        let tmp3 = d[at(3)] + d[at(4)];
        let tmp4 = d[at(3)] - d[at(4)];

        let tmp10 = tmp0 + tmp3;
        let tmp13 = tmp0 - tmp3;
        let tmp11 = tmp1 + tmp2;
        let tmp12 = tmp1 - tmp2;
        d[at(0)] = tmp10 + tmp11;
        d[at(4)] = tmp10 - tmp11;
        let z1 = (tmp12 + tmp13) * C4;
        d[at(2)] = tmp13 + z1;
        d[at(6)] = tmp13 - z1;

        let tmp10 = tmp4 + tmp5;
        let tmp11 = tmp5 + tmp6;
        let tmp12 = tmp6 + tmp7;
        let z5 = (tmp10 - tmp12) * C6;
        let z2 = C2_MINUS_C6 * tmp10 + z5;
        let z4 = C2_PLUS_C6 * tmp12 + z5;
        let z3 = tmp11 * C4;
        let z11 = tmp7 + z3;
        let z13 = tmp7 - z3;
        d[at(5)] = z13 + z2;
        d[at(3)] = z13 - z2;
        d[at(1)] = z11 + z4;
        d[at(7)] = z11 - z4;
    }
    for row in 0..8 {
        pass(d, row * 8, 1);
    }
    for col in 0..8 {
        pass(d, col, 8);
    }
}

/// One 8×8 block of a padded sample plane: level shift, DCT, quantise (`forward_DCT_float`).
fn transform(plane: &[u8], stride: usize, bx: usize, by: usize, div: &[f32; 64]) -> Block {
    let mut ws = [0f32; 64];
    for r in 0..8 {
        let row = &plane[(by * 8 + r) * stride + bx * 8..][..8];
        for c in 0..8 {
            ws[r * 8 + c] = (i32::from(row[c]) - 128) as f32;
        }
    }
    fdct_float(&mut ws);
    // fused: round(ws·div + 16384.5) toward zero, then remove the bias (`fmla` + `fcvtzs`).
    std::array::from_fn(|i| (ws[i].mul_add(div[i], 16384.5) as i32 - 16384) as i16)
}

/// A plane extended to `pw` × `ph` by repeating its last column and row.
fn pad(src: &[u8], w: usize, h: usize, pw: usize, ph: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(pw * ph);
    for y in 0..ph {
        let row = &src[y.min(h - 1) * w..][..w];
        out.extend_from_slice(row);
        out.extend(std::iter::repeat_n(row[w - 1], pw - w));
    }
    out
}

/// `h2v2_downsample` into a `ow` × `oh` plane: 2×2 means with the bias alternating 1, 2 along
/// each row; input past the edges repeats the last column/row, output rows past the image
/// repeat the last real one (`expand_bottom_edge`).
fn downsample(src: &[u8], w: usize, h: usize, ow: usize, oh: usize) -> Vec<u8> {
    let wide = pad(src, w, h, ow * 2, h);
    let real_rows = h.div_ceil(2);
    let mut out = Vec::with_capacity(ow * oh);
    for oy in 0..oh {
        let y = oy.min(real_rows - 1);
        let (r0, r1) = (&wide[(2 * y) * ow * 2..][..ow * 2], &wide[(2 * y + 1).min(h - 1) * ow * 2..][..ow * 2]);
        let mut bias = 1u16;
        for ox in 0..ow {
            let s = u16::from(r0[2 * ox]) + u16::from(r0[2 * ox + 1]) + u16::from(r1[2 * ox]) + u16::from(r1[2 * ox + 1]);
            out.push(((s + bias) >> 2) as u8);
            bias ^= 3;
        }
    }
    out
}

/// Number of magnitude bits of a coefficient (0 for 0).
fn nbits(v: i32) -> u32 {
    32 - v.unsigned_abs().leading_zeros()
}

/// Symbol statistics of one block (`htest_one_block`).
fn count_block(b: &Block, last_dc: i16, dc: &mut [u64; 257], ac: &mut [u64; 257]) {
    dc[nbits(i32::from(b[0]) - i32::from(last_dc)) as usize] += 1;
    let mut run = 0;
    for &k in &ZIGZAG[1..] {
        let v = b[k];
        if v == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            ac[0xF0] += 1;
            run -= 16;
        }
        ac[(run << 4) + nbits(i32::from(v)) as usize] += 1;
        run = 0;
    }
    if run > 0 {
        ac[0] += 1;
    }
}

/// A Huffman table as stored in DHT: counts per code length 1..=16 and the symbols.
struct Table {
    bits: [u8; 17],
    vals: Vec<u8>,
    code: [u16; 256],
    size: [u8; 256],
}

/// `jpeg_gen_optimal_table` (Annex K.2, with libjpeg's tie-breaking toward larger symbols and
/// the reserved pseudo-symbol 256), then the canonical codes (`jpeg_make_c_derived_tbl`).
fn optimal_table(freq: &[u64; 257]) -> Table {
    const MAX_CLEN: usize = 32;
    let mut freq = *freq;
    let mut bits = [0u8; MAX_CLEN + 1];
    let mut codesize = [0usize; 257];
    let mut others = [-1i32; 257];
    freq[256] = 1;
    loop {
        let mut c1 = None;
        let mut v = u64::MAX;
        for (i, &f) in freq.iter().enumerate() {
            if f != 0 && f <= v {
                v = f;
                c1 = Some(i);
            }
        }
        let mut c2 = None;
        v = u64::MAX;
        for (i, &f) in freq.iter().enumerate() {
            if f != 0 && f <= v && Some(i) != c1 {
                v = f;
                c2 = Some(i);
            }
        }
        let (Some(mut c1), Some(mut c2)) = (c1, c2) else { break };
        freq[c1] += freq[c2];
        freq[c2] = 0;
        codesize[c1] += 1;
        while others[c1] >= 0 {
            c1 = others[c1] as usize;
            codesize[c1] += 1;
        }
        others[c1] = c2 as i32;
        codesize[c2] += 1;
        while others[c2] >= 0 {
            c2 = others[c2] as usize;
            codesize[c2] += 1;
        }
    }
    for &s in &codesize {
        if s != 0 {
            bits[s] += 1;
        }
    }
    let mut i = MAX_CLEN;
    while i > 16 {
        while bits[i] > 0 {
            let mut j = i - 2;
            while bits[j] == 0 {
                j -= 1;
            }
            bits[i] -= 2;
            bits[i - 1] += 1;
            bits[j + 1] += 2;
            bits[j] -= 1;
        }
        i -= 1;
    }
    while bits[i] == 0 {
        i -= 1;
    }
    bits[i] -= 1;
    let mut vals = Vec::new();
    for len in 1..=MAX_CLEN {
        for (sym, &s) in codesize[..256].iter().enumerate() {
            if s == len {
                vals.push(sym as u8);
            }
        }
    }
    let mut t = Table { bits: [0; 17], vals, code: [0; 256], size: [0; 256] };
    t.bits.copy_from_slice(&bits[..17]);
    let n: usize = t.bits.iter().map(|&b| usize::from(b)).sum();
    t.vals.truncate(n);
    let mut code = 0u16;
    let mut p = 0;
    for len in 1..=16u8 {
        for _ in 0..t.bits[usize::from(len)] {
            let sym = usize::from(t.vals[p]);
            t.code[sym] = code;
            t.size[sym] = len;
            code += 1;
            p += 1;
        }
        code <<= 1;
    }
    t
}

/// Entropy-coded segment writer with 0xFF byte stuffing (`emit_bits`).
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, value: u32, size: u32) {
        let mut acc = (value & ((1 << size) - 1)) << (24 - self.n - size);
        acc |= self.acc;
        self.n += size;
        while self.n >= 8 {
            let c = (acc >> 16) as u8;
            self.out.push(c);
            if c == 0xFF {
                self.out.push(0);
            }
            acc <<= 8;
            self.n -= 8;
        }
        self.acc = acc & 0x00FF_FFFF;
    }

    /// Pads the last byte with 1 bits (`flush_bits`).
    fn flush(&mut self) {
        self.put(0x7F, 7);
        self.acc = 0;
        self.n = 0;
    }
}

/// One block (`encode_one_block`).
fn encode_block(w: &mut Bits, b: &Block, last_dc: i16, dc: &Table, ac: &Table) {
    let diff = i32::from(b[0]) - i32::from(last_dc);
    let n = nbits(diff);
    w.put(u32::from(dc.code[n as usize]), u32::from(dc.size[n as usize]));
    if n > 0 {
        w.put((if diff < 0 { diff - 1 } else { diff }) as u32, n);
    }
    let mut run = 0usize;
    for &k in &ZIGZAG[1..] {
        let v = i32::from(b[k]);
        if v == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            w.put(u32::from(ac.code[0xF0]), u32::from(ac.size[0xF0]));
            run -= 16;
        }
        let n = nbits(v);
        let sym = (run << 4) + n as usize;
        w.put(u32::from(ac.code[sym]), u32::from(ac.size[sym]));
        w.put((if v < 0 { v - 1 } else { v }) as u32, n);
        run = 0;
    }
    if run > 0 {
        w.put(u32::from(ac.code[0]), u32::from(ac.size[0]));
    }
}

fn marker(out: &mut Vec<u8>, code: u8, body: &[u8]) {
    out.extend_from_slice(&[0xFF, code]);
    out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(body);
}

/// Encodes `rgb` (`width` × `height` RGB triples, row-major) as a JPEG file.
pub fn encode(width: usize, height: usize, rgb: &[u8]) -> Vec<u8> {
    assert!(width > 0 && height > 0 && width <= 65535 && height <= 65535);
    assert_eq!(rgb.len(), width * height * 3);
    let (y, cb, cr) = rgb_to_ycc(rgb);
    let (mcu_x, mcu_y) = (width.div_ceil(16), height.div_ceil(16));

    // Luma: real blocks from the padded plane, then dummy blocks up to whole MCUs.
    let (ybw, ybh) = (width.div_ceil(8), height.div_ceil(8));
    let (yw, yh) = (mcu_x * 2, mcu_y * 2);
    let qy = quant_table(&LUMA);
    let qc = quant_table(&CHROMA);
    let (dy, dc) = (divisors(&qy), divisors(&qc));
    let yplane = pad(&y, width, height, ybw * 8, ybh * 8);
    let mut yblocks = vec![[0i16; 64]; yw * yh];
    for by in 0..ybh {
        for bx in 0..ybw {
            yblocks[by * yw + bx] = transform(&yplane, ybw * 8, bx, by, &dy);
        }
        // a dummy block at the right edge takes the DC of the last real block
        if ybw < yw {
            yblocks[by * yw + ybw][0] = yblocks[by * yw + ybw - 1][0];
        }
    }
    // a dummy block row at the bottom: each MCU's pair takes the DC of the last block of
    // the same MCU in the row above
    if ybh < yh {
        for m in 0..mcu_x {
            let last = yblocks[(ybh - 1) * yw + 2 * m + 1][0];
            yblocks[ybh * yw + 2 * m][0] = last;
            yblocks[ybh * yw + 2 * m + 1][0] = last;
        }
    }
    // Chroma: one block per MCU, all real.
    let (cw, ch) = (mcu_x * 8, mcu_y * 8);
    let chroma = |plane: &[u8]| -> Vec<Block> {
        let ds = downsample(plane, width, height, cw, ch);
        (0..mcu_y).flat_map(|by| (0..mcu_x).map(move |bx| (bx, by))).map(|(bx, by)| transform(&ds, cw, bx, by, &dc)).collect()
    };
    let (cbb, crb) = (chroma(&cb), chroma(&cr));

    // MCU order: four luma blocks (row-major), Cb, Cr.
    let mcu = |mx: usize, my: usize| -> [(usize, &Block); 6] {
        let at = |dx: usize, dy: usize| &yblocks[(my * 2 + dy) * yw + mx * 2 + dx];
        [(0, at(0, 0)), (0, at(1, 0)), (0, at(0, 1)), (0, at(1, 1)), (1, &cbb[my * mcu_x + mx]), (2, &crb[my * mcu_x + mx])]
    };

    // Pass 1: statistics (tables 0 for luma, 1 for both chroma components).
    let mut dcf = [[0u64; 257]; 2];
    let mut acf = [[0u64; 257]; 2];
    let mut last = [0i16; 3];
    for my in 0..mcu_y {
        for mx in 0..mcu_x {
            for (c, b) in mcu(mx, my) {
                let t = usize::from(c > 0);
                count_block(b, last[c], &mut dcf[t], &mut acf[t]);
                last[c] = b[0];
            }
        }
    }
    let tables = [[optimal_table(&dcf[0]), optimal_table(&acf[0])], [optimal_table(&dcf[1]), optimal_table(&acf[1])]];

    // Pass 2: headers and data.
    let mut out = vec![0xFF, 0xD8];
    marker(&mut out, 0xE0, &[b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]);
    for (id, q) in [(0u8, &qy), (1, &qc)] {
        let mut body = vec![id];
        body.extend(ZIGZAG.iter().map(|&k| q[k] as u8));
        marker(&mut out, 0xDB, &body);
    }
    let (h, w) = ((height as u16).to_be_bytes(), (width as u16).to_be_bytes());
    marker(&mut out, 0xC0, &[8, h[0], h[1], w[0], w[1], 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    for (id, [dct, act]) in tables.iter().enumerate() {
        for (class, t) in [(0u8, dct), (0x10, act)] {
            let mut body = vec![class | id as u8];
            body.extend_from_slice(&t.bits[1..]);
            body.extend_from_slice(&t.vals);
            marker(&mut out, 0xC4, &body);
        }
    }
    marker(&mut out, 0xDA, &[3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
    let mut bits = Bits { out, acc: 0, n: 0 };
    let mut last = [0i16; 3];
    for my in 0..mcu_y {
        for mx in 0..mcu_x {
            for (c, b) in mcu(mx, my) {
                let [dct, act] = &tables[usize::from(c > 0)];
                encode_block(&mut bits, b, last[c], dct, act);
                last[c] = b[0];
            }
        }
    }
    bits.flush();
    let mut out = bits.out;
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_to_the_input() {
        // odd sizes exercise the edge padding and the dummy blocks
        for (w, h) in [(80, 80), (37, 23), (17, 33), (1, 1)] {
            let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i * 7 % 256) as u8, (i % w * 255 / w) as u8, 128]).collect();
            let jpg = encode(w, h, &rgb);
            let img = image::load_from_memory(&jpg).unwrap().to_rgb8();
            assert_eq!((img.width() as usize, img.height() as usize), (w, h));
        }
    }

    #[test]
    fn quality_85_tables() {
        let q = quant_table(&LUMA);
        assert_eq!(&q[..4], &[5, 3, 3, 5]);
        assert_eq!(quant_table(&CHROMA)[63], 30);
    }
}
