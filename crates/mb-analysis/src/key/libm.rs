//! The float libm entries BALib's key code calls (`__sincosf_stret`, `exp2f`, `tanf`,
//! `log10f`); the compiler merges `sinf`/`cosf` pairs into `__sincosf_stret`, which is used
//! here directly so the results are the binary's.

#[cfg(target_os = "macos")]
mod sys {
    #[repr(C)]
    pub struct SinCosF {
        pub sin: f32,
        pub cos: f32,
    }
    unsafe extern "C" {
        pub fn __sincosf_stret(x: f32) -> SinCosF;
    }
}

/// `(sin x, cos x)`.
pub fn sincosf(x: f32) -> (f32, f32) {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: pure libm function, no pointers involved.
        let r = unsafe { sys::__sincosf_stret(x) };
        (r.sin, r.cos)
    }
    #[cfg(not(target_os = "macos"))]
    {
        (x.sin(), x.cos())
    }
}
