//! The reader interface shared by all formats, modelled on rekordbox's `rbxfrm::FileReader`
//! (`fileRead(int pos, Float2* dst, int count)`): random access by sample position, sequential
//! reads are cheap, positions outside `[0, length)` read as silence.
//!
//! [`DecReader`] ports `rbxfrm::AbstractFileReadDec::fileRead` @0x10280d3d8, the frame-based
//! reader that MP3 and AAC share (spec: `docs/re/decode.md` § Frame readers).

/// One stereo sample, as rekordbox's `rbxfrm::Float2`.
pub type Float2 = [f32; 2];

/// Static facts about a reader (rekordbox's `FileReader` fields 0x40 and 0x68…0x80).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderInfo {
    /// Sample rate of the decoded stream.
    pub sample_rate: u32,
    /// Channels in the file (1 or 2); mono is duplicated to both outputs.
    pub channels: u16,
    /// Bits per sample of the source (16 for MP3/AAC, the PCM width otherwise; 32 for float WAV).
    pub bits: u16,
    /// Length in samples per channel (rekordbox `getLength`).
    pub length: u32,
}

pub trait Reader: Send {
    fn info(&self) -> ReaderInfo;
    /// Fills `out` with the samples starting at `pos`; positions outside the stream are zero.
    fn read(&mut self, pos: i64, out: &mut [Float2]);
}

impl<R: Reader + ?Sized> Reader for Box<R> {
    fn info(&self) -> ReaderInfo {
        (**self).info()
    }
    fn read(&mut self, pos: i64, out: &mut [Float2]) {
        (**self).read(pos, out)
    }
}

/// One entry of rekordbox's `FileReader::FrameInfo` table (16 bytes: offset, sample position,
/// frame sample count, size | 0x80000000).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    /// Byte offset of the frame in the file.
    pub offset: u32,
    /// Sample position of the frame's first sample.
    pub pos: i32,
    /// Samples in the frame (MP4 `stts` delta; 0 for MP3, which uses a fixed frame size).
    pub samples: u32,
    /// Size in bytes.
    pub size: u32,
}

/// What a frame decoder produced for one `decode` call.
pub(crate) struct DecodeState {
    /// Index (in interleaved samples) of the next sample to hand out (`+0xac`).
    pub index: i32,
    /// Interleaved samples in `pcm` (`+0xb0`).
    pub len: i32,
    pub pcm: Vec<i16>,
}

impl DecodeState {
    pub fn new() -> Self {
        Self { index: 0, len: 0, pcm: Vec::new() }
    }
}

/// A frame decoder: the per-format half of `AbstractFileReadDec` (vtable slot 0xc8 `decode`).
pub(crate) trait FrameDecoder: Send {
    fn frames(&self) -> &[FrameInfo];
    fn info(&self) -> ReaderInfo;
    /// Scale applied to the 16-bit output (`+0x78`, 2^-15 for MP3 and AAC).
    fn scale(&self) -> f32;
    /// Next frame to output (`+0xa8`); −1 after construction.
    fn next_frame(&mut self) -> &mut i32;
    fn state(&mut self) -> &mut DecodeState;
    /// Decodes towards sample `pos`, starting at frame `first`; `reset` restarts the decoder.
    /// Returns false when nothing was produced.
    fn decode(&mut self, pos: i32, first: i32, reset: bool) -> bool;
}

/// `AbstractFileReadDec::fileRead` over a [`FrameDecoder`].
pub(crate) struct DecReader<D: FrameDecoder> {
    pub dec: D,
    /// Position the next sequential read is expected at (`+0xa4`), −1 initially.
    next_pos: i64,
}

impl<D: FrameDecoder> DecReader<D> {
    pub fn new(dec: D) -> Self {
        Self { dec, next_pos: -1 }
    }

    fn read_at(&mut self, pos: i32, out: &mut [Float2]) {
        let n = out.len();
        let mut pos = pos;
        if self.next_pos != i64::from(pos) {
            let frames = self.dec.frames();
            // Last frame starting at or before `pos` (−1 when none).
            let (mut lo, mut hi) = (0i32, frames.len() as i32 - 1);
            while lo <= hi {
                let mid = lo + (hi - lo) / 2;
                if frames[mid as usize].pos <= pos {
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
            let target = hi.max(0);
            if *self.dec.next_frame() != target {
                *self.dec.next_frame() = target;
                // Decoding restarts five frames early (bit reservoir / decoder warm-up).
                let back = hi.max(5);
                if !self.dec.decode(pos, back - 5, true) {
                    let frames = self.dec.frames();
                    if ((back - 4) as usize) < frames.len() {
                        let gap = (frames[(back - 4) as usize].pos - frames[(back - 5) as usize].pos).max(0) as usize;
                        if gap <= n {
                            out[..gap].fill([0.0; 2]);
                            if n <= gap {
                                return;
                            }
                            self.next_pos = -1;
                            *self.dec.next_frame() = -1;
                            return self.read_at(pos + gap as i32, &mut out[gap..]);
                        }
                    }
                    out.fill([0.0; 2]);
                    return;
                }
            }
        }
        let info = self.dec.info();
        let ch = i32::from(info.channels);
        let total = info.length as i32;
        let scale = self.dec.scale();
        let mut k = 0;
        while k < n {
            if pos < total {
                if self.dec.state().index >= self.dec.state().len {
                    let next = *self.dec.next_frame();
                    if !self.dec.decode(pos, next, false) {
                        out[k..].fill([0.0; 2]);
                        return;
                    }
                }
                let st = self.dec.state();
                if st.index < 0 {
                    out[k] = [0.0; 2];
                } else if st.len <= st.index {
                    // Nothing decoded for this position: silence, and retry with the next frame.
                    out[k] = [0.0; 2];
                    pos += 1;
                    k += 1;
                    continue;
                } else {
                    let i = st.index as usize;
                    let l = f32::from(st.pcm[i]);
                    let r = f32::from(st.pcm[i + (ch as usize) - 1]);
                    out[k] = [l * scale, r * scale];
                }
                st.index += ch;
            } else {
                out[k] = [0.0; 2];
            }
            pos += 1;
            k += 1;
        }
        self.next_pos = i64::from(pos);
    }
}

impl<D: FrameDecoder> Reader for DecReader<D> {
    fn info(&self) -> ReaderInfo {
        self.dec.info()
    }

    fn read(&mut self, pos: i64, out: &mut [Float2]) {
        let pos = pos.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        self.read_at(pos, out);
    }
}
