//! Timeline conversions between samples, milliseconds and the 1/150 s columns of the scrolling
//! waveforms (PWV3/PWV5/PWV7). Spec: `docs/re/decode.md` § Timeline.
//!
//! rekordbox's analysis rate is always 44.1 or 48 kHz ([`crate::analysis_rate`]), where a 150 fps
//! column is exactly 294 or 320 samples. Positions in ANLZ files (PQTZ beats, PCOB/PCO2 cues) are
//! whole milliseconds of that timeline.

/// Waveform columns per second (PWV3/PWV5/PWV7 header field).
pub const COLUMNS_PER_SECOND: u32 = 150;

/// Samples per waveform column at `rate` (294 at 44.1 kHz, 320 at 48 kHz).
pub fn samples_per_column(rate: u32) -> u32 {
    rate / COLUMNS_PER_SECOND
}

/// Samples the analysis actually reads: rekordbox stops one sample short of the decoded length
/// (`CBeatAnalyzeThread::analyze` @0x1001528c4 analyses positions `0 .. length−1`).
pub fn analysed_samples(total_samples: u64) -> u64 {
    total_samples.saturating_sub(1)
}

/// Number of scrolling-waveform columns rekordbox writes for a track of `total_samples` at `rate`:
/// ⌈(length − 1) / samples_per_column⌉. Matches PWV3 on every oracle-a track.
pub fn waveform_columns(total_samples: u64, rate: u32) -> u64 {
    let spc = u64::from(samples_per_column(rate).max(1));
    analysed_samples(total_samples).div_ceil(spc)
}

/// Column containing `sample`.
pub fn sample_to_column(sample: u64, rate: u32) -> u64 {
    sample / u64::from(samples_per_column(rate).max(1))
}

/// First sample of `column`.
pub fn column_to_sample(column: u64, rate: u32) -> u64 {
    column * u64::from(samples_per_column(rate))
}

/// Exact time of `sample` in milliseconds.
pub fn sample_to_ms(sample: i64, rate: u32) -> f64 {
    sample as f64 * 1000.0 / f64::from(rate)
}

/// Whole milliseconds (truncated) and the microsecond remainder, the split PCP2/PQT2 store.
pub fn sample_to_ms_us(sample: u64, rate: u32) -> (u32, u16) {
    let us = sample * 1_000_000 / u64::from(rate);
    ((us / 1000) as u32, (us % 1000) as u16)
}

/// Nearest sample to `ms`.
pub fn ms_to_sample(ms: f64, rate: u32) -> i64 {
    (ms * f64::from(rate) / 1000.0).round() as i64
}

/// Time of the start of `column` in milliseconds.
pub fn column_to_ms(column: u64) -> f64 {
    column as f64 * 1000.0 / f64::from(COLUMNS_PER_SECOND)
}

/// Column containing time `ms`.
pub fn ms_to_column(ms: f64) -> u64 {
    (ms.max(0.0) * f64::from(COLUMNS_PER_SECOND) / 1000.0).floor() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns() {
        assert_eq!(samples_per_column(44100), 294);
        assert_eq!(samples_per_column(48000), 320);
        // 60 s at 44.1 kHz → 9000 columns (oracle-a WAV probes); 2298 MP3 frames → 9005.
        assert_eq!(waveform_columns(2_646_000, 44100), 9000);
        assert_eq!(waveform_columns(2_647_296, 44100), 9005);
        assert_eq!(waveform_columns(2_880_000, 48000), 9000);
        assert_eq!(sample_to_column(293, 44100), 0);
        assert_eq!(sample_to_column(294, 44100), 1);
        assert_eq!(column_to_ms(150), 1000.0);
        assert_eq!(ms_to_column(1000.0), 150);
    }

    #[test]
    fn ms() {
        assert_eq!(sample_to_ms(44100, 44100), 1000.0);
        assert_eq!(ms_to_sample(1234.0, 44100), 54419);
        assert_eq!(sample_to_ms_us(54419, 44100), (1233, 990));
    }
}
