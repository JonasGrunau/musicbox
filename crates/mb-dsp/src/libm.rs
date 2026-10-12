//! libm calls the way rekordbox makes them. rekordbox links the system libm; on macOS the
//! compiler merges `sin(x)` and `cos(x)` into one `__sincos_stret` call, which is what the
//! filter designs use (`analysis-waveforms.md` § 4.1). Its results can differ from separate
//! `sin`/`cos` calls by one ulp, so the call itself has to be the same.

#[cfg(target_os = "macos")]
mod sys {
    #[repr(C)]
    pub struct SinCos {
        pub sin: f64,
        pub cos: f64,
    }
    unsafe extern "C" {
        pub fn __sincos_stret(x: f64) -> SinCos;
        pub fn __exp10(x: f64) -> f64;
    }
}

/// `(sin x, cos x)` from the same libm entry rekordbox uses.
pub fn sincos(x: f64) -> (f64, f64) {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: pure libm function, no pointers involved.
        let r = unsafe { sys::__sincos_stret(x) };
        (r.sin, r.cos)
    }
    #[cfg(not(target_os = "macos"))]
    {
        (x.sin(), x.cos())
    }
}

/// `tan` (system libm, like rekordbox).
pub fn tan(x: f64) -> f64 {
    x.tan()
}

/// `10^x` through the libm entry rekordbox calls (`__exp10` on macOS; a `powf` fallback
/// elsewhere, which may differ in the last bit).
pub fn exp10(x: f64) -> f64 {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: pure libm function, no pointers involved.
        unsafe { sys::__exp10(x) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        10f64.powf(x)
    }
}

#[cfg(test)]
mod tests {
    /// `__sincos_stret` is not always bit-identical to separate `sin`/`cos` calls (it differs by
    /// one ulp at x = −0.727, for example), which is why the filter designs call it and not
    /// `f64::sin_cos`. Both stay within one ulp of each other.
    #[test]
    fn sincos_within_one_ulp_of_separate_calls() {
        let mut differ = 0;
        for i in 0..2000 {
            let x = f64::from(i) * 0.0123 - 7.0;
            let (s, c) = super::sincos(x);
            for (a, b) in [(s, x.sin()), (c, x.cos())] {
                let d = (a.to_bits() as i64 - b.to_bits() as i64).abs();
                assert!(d <= 1, "{x}: {a} vs {b}");
                differ += usize::from(d != 0);
            }
        }
        eprintln!("{differ} of 4000 values differ by one ulp");
    }
}
