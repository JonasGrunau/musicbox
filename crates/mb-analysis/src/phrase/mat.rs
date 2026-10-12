//! The `MatArrayFloat` / `MatArrayInt` helpers the phrase analyser calls, with their summation
//! order (`analysis-phrase.md` § 3.2).

/// `MatArrayFloat::sum` @0x100dfacc8: up to 7 values in order; longer arrays in 8 lanes
/// reduced as `((a6 + (a4 + (a2 + a0))) + (a7 + (a5 + (a3 + a1))))`, then the tail in order.
pub fn sum(x: &[f64]) -> f64 {
    lanes(x, |acc, v| v + acc)
}

/// `MatArrayFloat::square_sum` @0x100dfae48: like [`sum`] with fused `acc + v·v`.
pub fn square_sum(x: &[f64]) -> f64 {
    lanes(x, |acc, v| v.mul_add(v, acc))
}

fn lanes(x: &[f64], f: impl Fn(f64, f64) -> f64) -> f64 {
    let n = x.len();
    if n == 0 {
        return 0.0;
    }
    let mut acc = 0.0;
    let mut i = 0;
    if n - 1 >= 7 {
        let mut a = [0.0f64; 8];
        let full = n & !7;
        for blk in x[..full].chunks_exact(8) {
            for (l, v) in a.iter_mut().zip(blk) {
                *l = f(*l, *v);
            }
        }
        acc = (a[6] + (a[4] + (a[2] + a[0]))) + (a[7] + (a[5] + (a[3] + a[1])));
        i = full;
    }
    for &v in &x[i..] {
        acc = f(acc, v);
    }
    acc
}

/// `MatArrayFloat::max_idx`: the first largest value and its index.
pub fn max_idx(x: &[f64]) -> (f64, usize) {
    let mut best = 0;
    for (i, &v) in x.iter().enumerate().skip(1) {
        if !(v <= x[best]) {
            best = i;
        }
    }
    (x.get(best).copied().unwrap_or(0.0), best)
}

/// `MatArrayFloat::max_val`.
pub fn max_val(x: &[f64]) -> f64 {
    max_idx(x).0
}

/// `MatArrayFloat::min_val`: the first smallest value.
pub fn min_val(x: &[f64]) -> f64 {
    let mut best = match x.first() {
        Some(&v) => v,
        None => return 0.0,
    };
    for &v in &x[1..] {
        if !(best <= v) {
            best = v;
        }
    }
    best
}

/// `MatArrayFloat::sort_descend_idx(1)`: sorts the values descending (stable) and returns the
/// 1-based original positions.
pub fn sort_descend_idx(x: &mut [f64]) -> Vec<i32> {
    let mut v: Vec<(f64, i32)> = x.iter().enumerate().map(|(i, &v)| (v, i as i32 + 1)).collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (d, s) in x.iter_mut().zip(&v) {
        *d = s.0;
    }
    v.into_iter().map(|s| s.1).collect()
}

/// `MatArrayInt::sort_ascend_idx(1)`: sorts ascending (stable) and returns the 1-based
/// original positions.
pub fn sort_ascend_idx(x: &mut [i32]) -> Vec<i32> {
    let mut v: Vec<(i32, i32)> = x.iter().enumerate().map(|(i, &v)| (v, i as i32 + 1)).collect();
    v.sort_by_key(|s| s.0);
    for (d, s) in x.iter_mut().zip(&v) {
        *d = s.0;
    }
    v.into_iter().map(|s| s.1).collect()
}

/// C's `%` on `i32` (truncating), as `sdiv` + `msub` compute it.
pub fn crem(a: i32, b: i32) -> i32 {
    if b == 0 { a } else { a.wrapping_rem(b) }
}

/// C's `/` on `i32`, with the binaries' `b == 0 → 0` guard.
pub fn cdiv(a: i32, b: i32) -> i32 {
    if b == 0 { 0 } else { a.wrapping_div(b) }
}
