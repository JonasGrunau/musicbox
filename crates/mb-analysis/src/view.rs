//! The player's zoomed waveform: what rekordbox draws in a deck's zoomed strip in the RGB style.
//! It is not stored in the analysis files. rekordbox builds it from the audio when a track loads
//! (`djplay::WaveZoom::readWaveZoom` @0x10198b4c4: WaveCreator's mono mix, filters and record A,
//! one entry per millisecond) and draws it with `djplay::WaveView::createWaveInfo` @0x1021916c0:
//! the height is the linear peak level, the colour comes from the three band shares.
//! Spec: `analysis-waveforms.md` § 7.

use crate::{Error, Result, block_len, wavecreator};
use std::path::Path;

/// One millisecond of the zoomed waveform.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewMs {
    /// Peak level of the mono mix, `max(|max|, |min|)` in Q15 (32768 = full scale).
    pub peak: u16,
    /// Low, mid and high band shares (record A). The low is the largest within ±12 ms and the mid
    /// the largest of the previous, this and the next millisecond, as in the PWV4 points.
    pub bands: [u16; 3],
}

/// The zoomed waveform of a whole track.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZoomView {
    /// One entry per millisecond from the start of the track.
    pub ms: Vec<ViewMs>,
}

impl ZoomView {
    /// The view from WaveCreator's record A ([`wavecreator::WaveCreator::into_record_a`]).
    pub fn from_record_a(rec: &[[i16; 6]]) -> Self {
        let n = rec.len();
        let share = |i: usize, f: usize| rec[i][f].max(0) as u16;
        let ms = (0..n)
            .map(|i| {
                let low = (i.saturating_sub(12)..(i + 13).min(n)).map(|j| share(j, 3)).max().unwrap_or(0);
                let mid = (i.saturating_sub(1)..(i + 2).min(n)).map(|j| share(j, 4)).max().unwrap_or(0);
                let peak = i32::from(rec[i][0]).abs().max(i32::from(rec[i][1]).abs()) as u16;
                ViewMs { peak, bands: [low, mid, share(i, 5)] }
            })
            .collect();
        Self { ms }
    }

    /// The RGB colour rekordbox gives a column with these band values (`createWaveInfo` uses the
    /// same formula as the PWV5 colours, `WaveCreator::calcZoomWaveColour`); white when all are 0.
    pub fn colour(bands: [u16; 3]) -> [u8; 3] {
        let (r, g, b) = wavecreator::zoom_colour(f32::from(bands[0]), f32::from(bands[1]), f32::from(bands[2]));
        [r, g, b]
    }
}

/// Builds the zoomed waveform of the audio file at `path`, reading it as the analysis does
/// (positions `0 .. L − 1`, 16-bit samples, two-second blocks).
pub fn zoom_view(path: impl AsRef<Path>) -> Result<ZoomView> {
    let mut dec = mb_audio::open(path.as_ref())?;
    let info = *dec.info();
    let len = info.total_samples;
    if len < 2 {
        return Err(Error::Empty);
    }
    let total = len - 1;
    let mut wave = wavecreator::WaveCreator::new_view(info.sample_rate, len);
    let block = block_len(info.sample_rate);
    let mut frames = vec![[0.0f32; 2]; block];
    let mut pcm = vec![[0i16; 2]; block];
    let mut pos = 0u64;
    while pos < total {
        let n = ((total - pos) as usize).min(block);
        let got = dec.read_frames(&mut frames[..n]);
        if got == 0 {
            break;
        }
        for (dst, src) in pcm[..got].iter_mut().zip(&frames[..got]) {
            *dst = [mb_audio::analysis_i16(src[0]), mb_audio::analysis_i16(src[1])];
        }
        wave.put(&pcm[..got]);
        pos += got as u64;
    }
    Ok(ZoomView::from_record_a(&wave.into_record_a()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_and_peak() {
        let mut rec = vec![[0i16; 6]; 40];
        rec[20] = [-3, -9000, 0, 500, 300, 7];
        let v = ZoomView::from_record_a(&rec);
        assert_eq!(v.ms[20].peak, 9000);
        // The low reaches 12 ms either side, the mid one.
        assert_eq!((v.ms[8].bands[0], v.ms[7].bands[0], v.ms[32].bands[0], v.ms[33].bands[0]), (500, 0, 500, 0));
        assert_eq!((v.ms[19].bands[1], v.ms[18].bands[1], v.ms[21].bands[1]), (300, 0, 300));
        assert_eq!((v.ms[20].bands[2], v.ms[21].bands[2]), (7, 0));
        assert_eq!(ZoomView::colour([0, 0, 0]), [255, 255, 255]);
    }
}
