//! DSP primitives as rekordbox 7.2.19's analysis computes them, down to the last bit: the same
//! operation order, the same fused multiply-adds (Rust `mul_add` lowers to `fmadd`/`fmsub`), and
//! the same libm calls. Spec: `docs/re/analysis-waveforms.md`.

pub mod biquad;
pub mod iir;
pub mod libm;

pub use biquad::Biquad;
pub use iir::IirPassFilter;
