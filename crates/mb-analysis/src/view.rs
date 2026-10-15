//! The player's zoomed waveform: what rekordbox draws in a deck's zoomed strip in the RGB style.
//! It is not stored in the analysis files. rekordbox builds it from the audio when a track loads
//! (`djplay::WaveZoom::readWaveZoom` @0x10198b4c4: WaveCreator's mono mix, filters and record A,
//! one entry per millisecond) and draws it with `djplay::WaveView::createWaveInfo` @0x1021916c0:
//! the height is the linear peak level, the colour comes from the three band shares.
//! Spec: `analysis-waveforms.md` § 7.

use crate::{Error, Result, block_len, wavecreator};
use std::ops::ControlFlow;
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

/// The zoomed waveform of a track, or of its start while it is being built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZoomView {
    /// One entry per millisecond from the start of the track.
    pub ms: Vec<ViewMs>,
}

/// Millisecond `i` of the view from record A of a track of `rec.len()` milliseconds; reads
/// record A up to `i + 12`.
fn view_ms(rec: &[[i16; 6]], i: usize) -> ViewMs {
    let n = rec.len();
    let share = |j: usize, f: usize| rec[j][f].max(0) as u16;
    let low = (i.saturating_sub(12)..(i + 13).min(n)).map(|j| share(j, 3)).max().unwrap_or(0);
    let mid = (i.saturating_sub(1)..(i + 2).min(n)).map(|j| share(j, 4)).max().unwrap_or(0);
    let peak = i32::from(rec[i][0]).abs().max(i32::from(rec[i][1]).abs()) as u16;
    ViewMs { peak, bands: [low, mid, share(i, 5)] }
}

impl ZoomView {
    /// The view from a whole track's record A ([`wavecreator::WaveCreator::record_a`]).
    pub fn from_record_a(rec: &[[i16; 6]]) -> Self {
        Self { ms: (0..rec.len()).map(|i| view_ms(rec, i)).collect() }
    }

    /// The RGB colour rekordbox gives a column with these band values (`createWaveInfo` uses the
    /// same formula as the PWV5 colours, `WaveCreator::calcZoomWaveColour`); white when all are 0.
    pub fn colour(bands: [u16; 3]) -> [u8; 3] {
        let (r, g, b) = wavecreator::zoom_colour(f32::from(bands[0]), f32::from(bands[1]), f32::from(bands[2]));
        [r, g, b]
    }
}

/// Builds the zoomed waveform of the audio file at `path` from the start, reading it as the
/// analysis does (positions `0 .. L − 1`, 16-bit samples, two-second blocks). After each block
/// `sink` gets the milliseconds finished since its last call; returning `Break` stops the build.
pub fn build_zoom_view(path: impl AsRef<Path>, mut sink: impl FnMut(&[ViewMs]) -> ControlFlow<()>) -> Result<()> {
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
    let (mut pos, mut sent) = (0u64, 0usize);
    let mut chunk = Vec::new();
    loop {
        let n = ((total.saturating_sub(pos)) as usize).min(block);
        let got = if n == 0 { 0 } else { dec.read_frames(&mut frames[..n]) };
        if got > 0 {
            for (dst, src) in pcm[..got].iter_mut().zip(&frames[..got]) {
                *dst = [mb_audio::analysis_i16(src[0]), mb_audio::analysis_i16(src[1])];
            }
            wave.put(&pcm[..got]);
            pos += got as u64;
        }
        let rec = wave.record_a();
        // A millisecond is final once the ±12 ms low window lies within what is summarised.
        let end = if got == 0 { rec.len() } else { wave.done_ms().saturating_sub(12).min(rec.len()) };
        chunk.clear();
        chunk.extend((sent..end).map(|i| view_ms(rec, i)));
        sent = sent.max(end);
        if (!chunk.is_empty() && sink(&chunk).is_break()) || got == 0 {
            return Ok(());
        }
    }
}

/// [`build_zoom_view`] into one [`ZoomView`].
pub fn zoom_view(path: impl AsRef<Path>) -> Result<ZoomView> {
    let mut view = ZoomView::default();
    build_zoom_view(path, |ms| {
        view.ms.extend_from_slice(ms);
        ControlFlow::Continue(())
    })?;
    Ok(view)
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
