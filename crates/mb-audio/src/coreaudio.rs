//! AAC and ALAC in MP4 through macOS AudioToolbox.
//!
//! AAC ports `rbxfrm::FileReadMp4` (`decoderOpen` @0x102820c5c, `decode` @0x102820fe0,
//! `converterCallback` @0x102821154): `AudioFileOpenURL`, an `AudioConverter` from the file's data
//! format to interleaved signed 16-bit PCM, the magic cookie, and — when the file has a packet
//! table — `kAudioConverterPrimeInfo` set to {0, 0}. Each `AudioConverterFillComplexBuffer` call
//! asks for one packet's worth of frames and the input callback hands out exactly one packet, so
//! the output starts with the encoder priming (no trimming) and sample *k* of packet *i* sits at
//! the packet's `stts` position + *k*.
//!
//! rekordbox decodes ALAC with Apple's open-source `ALACDecoder` compiled into the binary
//! (`FileReadAlac::decode` @0x102818838). ALAC is lossless, so AudioToolbox's ALAC decoder yields
//! the same integers; the timeline is again the `stts` positions. Spec: `docs/re/decode.md` § MP4.

use crate::mp4::{Codec, Mp4Track};
use crate::reader::{DecodeState, Float2, FrameDecoder, FrameInfo, Reader, ReaderInfo};
use crate::{Error, Result};
use std::ffi::c_void;
use std::path::Path;

type OSStatus = i32;
type AudioFileID = *mut c_void;
type AudioConverterRef = *mut c_void;
type CFURLRef = *const c_void;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Asbd {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct PacketDesc {
    start_offset: i64,
    variable_frames: u32,
    data_byte_size: u32,
}

#[repr(C)]
struct AudioBuffer {
    channels: u32,
    byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    count: u32,
    buffers: [AudioBuffer; 1],
}

#[repr(C)]
#[derive(Default)]
struct PacketTableInfo {
    valid_frames: i64,
    priming_frames: i32,
    remainder_frames: i32,
}

type InputProc = extern "C" fn(AudioConverterRef, *mut u32, *mut AudioBufferList, *mut *mut PacketDesc, *mut c_void) -> OSStatus;

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioFileOpenURL(url: CFURLRef, perms: i8, hint: u32, out: *mut AudioFileID) -> OSStatus;
    fn AudioFileClose(f: AudioFileID) -> OSStatus;
    fn AudioFileGetProperty(f: AudioFileID, id: u32, size: *mut u32, data: *mut c_void) -> OSStatus;
    fn AudioFileGetPropertyInfo(f: AudioFileID, id: u32, size: *mut u32, writable: *mut u32) -> OSStatus;
    fn AudioFileReadPacketData(
        f: AudioFileID,
        use_cache: u8,
        io_bytes: *mut u32,
        descs: *mut PacketDesc,
        start: i64,
        io_packets: *mut u32,
        buffer: *mut c_void,
    ) -> OSStatus;
    fn AudioConverterNew(src: *const Asbd, dst: *const Asbd, out: *mut AudioConverterRef) -> OSStatus;
    fn AudioConverterDispose(c: AudioConverterRef) -> OSStatus;
    fn AudioConverterReset(c: AudioConverterRef) -> OSStatus;
    fn AudioConverterSetProperty(c: AudioConverterRef, id: u32, size: u32, data: *const c_void) -> OSStatus;
    fn AudioConverterFillComplexBuffer(
        c: AudioConverterRef,
        proc_: InputProc,
        user: *mut c_void,
        io_frames: *mut u32,
        out: *mut AudioBufferList,
        out_desc: *mut PacketDesc,
    ) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(alloc: *const c_void, buf: *const u8, len: isize, is_dir: u8) -> CFURLRef;
    fn CFRelease(cf: *const c_void);
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const LPCM: u32 = fourcc(b"lpcm");
/// kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked (rekordbox's constant @0x1038d8840).
const SIGNED_PACKED: u32 = 0x0c;

/// The input side shared with the converter callback (rekordbox: `this + 0x128`).
struct Feed {
    file: AudioFileID,
    /// Packet table: frames per packet for each packet.
    packet_frames: Vec<u32>,
    default_frames: u32,
    /// Bytes per output frame (channels × bytes per sample), for the packet cap.
    out_bytes_per_frame: u32,
    src_channels: u32,
    /// Next packet the callback will read (`+0x130`).
    next_packet: i64,
    desc: [PacketDesc; 512],
    buffer: Vec<u8>,
    /// Offer the whole buffer to `AudioFileReadPacketData` instead of `frames × bytes per frame`.
    /// ALAC needs it: an incompressible packet (escape mode) is a few bytes larger than its PCM,
    /// and a budget of exactly the PCM size makes the read fail (and every later packet with it).
    /// rekordbox decodes ALAC with its own `ALACDecoder`, not through this callback.
    whole_buffer: bool,
}

/// `FileReadMp4::converterCallback`: one packet per call, read with `AudioFileReadPacketData`.
extern "C" fn input_proc(
    _c: AudioConverterRef,
    io_packets: *mut u32,
    data: *mut AudioBufferList,
    out_desc: *mut *mut PacketDesc,
    user: *mut c_void,
) -> OSStatus {
    // SAFETY: `user` is the Feed owned by the converter's owner for the converter's lifetime;
    // the pointers come from AudioToolbox and are valid for this call.
    unsafe {
        let feed = &mut *user.cast::<Feed>();
        let data = &mut *data;
        let idx = feed.next_packet;
        if idx < 0 || idx as usize >= feed.packet_frames.len() {
            data.buffers[0].channels = 0;
            data.buffers[0].byte_size = 0;
            data.buffers[0].data = std::ptr::null_mut();
            if !out_desc.is_null() {
                *out_desc = std::ptr::null_mut();
            }
            return 1;
        }
        let mut frames = feed.packet_frames[idx as usize];
        if frames < 2 {
            frames = feed.default_frames;
        }
        let mut bytes = feed.out_bytes_per_frame * frames;
        if !io_packets.is_null() {
            let cap = 0x800 / feed.out_bytes_per_frame.max(1);
            if cap < *io_packets {
                *io_packets = cap;
            }
            *io_packets = (*io_packets).min(feed.desc.len() as u32);
        }
        if feed.whole_buffer {
            bytes = feed.buffer.len() as u32;
        }
        bytes = bytes.min(feed.buffer.len() as u32);
        let st = AudioFileReadPacketData(
            feed.file,
            1,
            &mut bytes,
            feed.desc.as_mut_ptr(),
            idx,
            io_packets,
            feed.buffer.as_mut_ptr().cast(),
        );
        if st != 0 {
            data.buffers[0].channels = 0;
            data.buffers[0].byte_size = 0;
            data.buffers[0].data = std::ptr::null_mut();
            if !out_desc.is_null() {
                *out_desc = std::ptr::null_mut();
            }
            return st;
        }
        data.buffers[0].data = feed.buffer.as_mut_ptr().cast();
        data.buffers[0].channels = feed.src_channels;
        data.buffers[0].byte_size = bytes;
        if !out_desc.is_null() {
            *out_desc = feed.desc.as_mut_ptr();
        }
        feed.next_packet += 1;
        0
    }
}

/// An open AudioFile plus converter.
struct Converter {
    file: AudioFileID,
    conv: AudioConverterRef,
    feed: Box<Feed>,
    src: Asbd,
    bytes_per_sample: u32,
    channels: u32,
}

// The AudioToolbox objects are used from one thread at a time.
unsafe impl Send for Converter {}

impl Drop for Converter {
    fn drop(&mut self) {
        // SAFETY: objects created in `open` and not used afterwards.
        unsafe {
            if !self.conv.is_null() {
                AudioConverterDispose(self.conv);
            }
            if !self.file.is_null() {
                AudioFileClose(self.file);
            }
        }
    }
}

fn status(what: &str, st: OSStatus) -> Result<()> {
    if st == 0 { Ok(()) } else { Err(Error::Format(format!("{what} failed ({st})"))) }
}

impl Converter {
    /// Opens the file and builds the converter. `out_bits` 16 for AAC (rekordbox's client format),
    /// the source bit depth for ALAC. `zero_priming` applies rekordbox's PrimeInfo {0, 0}.
    fn open(path: &Path, packet_frames: Vec<u32>, rate: Option<f64>, out_bits: Option<u32>, zero_priming: bool) -> Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: AudioToolbox / CoreFoundation calls with valid pointers; every object created
        // here is owned by the returned value or released before returning.
        unsafe {
            let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, 0);
            if url.is_null() {
                return Err(Error::Format("CFURL failed".into()));
            }
            let mut file: AudioFileID = std::ptr::null_mut();
            let st = AudioFileOpenURL(url, 1, 0, &mut file);
            CFRelease(url);
            status("AudioFileOpenURL", st)?;
            let mut me = Self {
                file,
                conv: std::ptr::null_mut(),
                feed: Box::new(Feed {
                    file,
                    packet_frames,
                    default_frames: 0,
                    out_bytes_per_frame: 0,
                    src_channels: 0,
                    next_packet: 0,
                    desc: [PacketDesc::default(); 512],
                    buffer: vec![0; 1 << 16],
                    whole_buffer: false,
                }),
                src: Asbd::default(),
                bytes_per_sample: 2,
                channels: 0,
            };
            let mut size = std::mem::size_of::<Asbd>() as u32;
            status("dfmt", AudioFileGetProperty(file, fourcc(b"dfmt"), &mut size, (&raw mut me.src).cast()))?;
            let ch = me.src.channels_per_frame;
            if !(1..=2).contains(&ch) {
                return Err(Error::Format(format!("{ch} channels")));
            }
            let bits = out_bits.unwrap_or(16);
            let bps = bits.div_ceil(8);
            let dst = Asbd {
                sample_rate: rate.unwrap_or(me.src.sample_rate),
                format_id: LPCM,
                format_flags: SIGNED_PACKED,
                bytes_per_packet: bps * ch,
                frames_per_packet: 1,
                bytes_per_frame: bps * ch,
                channels_per_frame: ch,
                bits_per_channel: bits,
                reserved: 0,
            };
            status("AudioConverterNew", AudioConverterNew(&me.src, &dst, &mut me.conv))?;
            let mut cookie_size: u32 = 4;
            let mut writable: u32 = 0;
            AudioFileGetPropertyInfo(file, fourcc(b"mgic"), &mut cookie_size, &mut writable);
            if cookie_size != 0 {
                let mut cookie = vec![0u8; cookie_size as usize];
                status("mgic", AudioFileGetProperty(file, fourcc(b"mgic"), &mut cookie_size, cookie.as_mut_ptr().cast()))?;
                status(
                    "dmgc",
                    AudioConverterSetProperty(me.conv, fourcc(b"dmgc"), cookie_size, cookie.as_ptr().cast()),
                )?;
            }
            if zero_priming {
                let mut info = PacketTableInfo::default();
                let mut size = 16u32;
                if AudioFileGetProperty(file, fourcc(b"pnfo"), &mut size, (&raw mut info).cast()) == 0 {
                    let prime: [u32; 2] = [0, 0];
                    status("prim", AudioConverterSetProperty(me.conv, fourcc(b"prim"), 8, prime.as_ptr().cast()))?;
                }
            }
            me.feed.default_frames = me.src.frames_per_packet;
            me.feed.out_bytes_per_frame = bps * ch;
            me.feed.src_channels = ch;
            me.bytes_per_sample = bps;
            me.channels = ch;
            Ok(me)
        }
    }

    fn reset(&mut self, packet: i64) {
        // SAFETY: valid converter.
        unsafe { AudioConverterReset(self.conv) };
        self.feed.next_packet = packet;
    }

    /// One `AudioConverterFillComplexBuffer` call for `frames` output frames into `out`.
    /// Returns (status, frames produced, bytes produced).
    fn fill(&mut self, frames: u32, out: &mut Vec<u8>) -> (OSStatus, u32, u32) {
        let bytes = frames * self.bytes_per_sample * self.channels;
        out.resize(bytes as usize, 0);
        let mut list = AudioBufferList {
            count: 1,
            buffers: [AudioBuffer { channels: self.channels, byte_size: bytes, data: out.as_mut_ptr().cast() }],
        };
        let mut io = frames;
        let user: *mut Feed = &mut *self.feed;
        // SAFETY: the buffer list points at `out`, sized for `frames`; the callback gets the Feed.
        let st = unsafe { fill_complex_buffer(self.conv, user.cast(), &mut io, &mut list) };
        (st, io, list.buffers[0].byte_size)
    }
}

/// The stack pointer (modulo 1024) at which `AudioConverterFillComplexBuffer` is entered.
///
/// Apple's AAC decoder does not give the same samples for every stack alignment: entered with
/// SP ≡ 48 (mod 64) it decodes slightly differently (a few samples ±1) than with SP ≡ 0/16/32,
/// on every file tested; the setup calls (`AudioConverterNew`, `dmgc`, `prim`) do not matter
/// (`docs/re/decode.md` § 6). rekordbox calls it from `FileReadMp4::decode` on its analysis
/// thread, at a fixed alignment, so its decodes are stable; ours followed wherever the caller's
/// stack happened to be. The call therefore goes through a trampoline that enters the function
/// at a fixed alignment.
fn fill_sp() -> usize {
    static SP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *SP.get_or_init(|| {
        std::env::var("MB_AAC_FILL_SP").ok().and_then(|v| v.parse().ok()).map_or(FILL_SP, |v: usize| v & 0x3f0)
    })
}

/// See [`fill_sp`].
const FILL_SP: usize = 0;

#[cfg(target_arch = "aarch64")]
std::arch::global_asm!(
    ".private_extern _mb_audio_call_at",
    ".globl _mb_audio_call_at",
    ".p2align 2",
    // x0..x5: arguments, x6: function, x7: stack pointer at the callee's entry modulo 1024
    // (a multiple of 16). Uses at most 3 KiB of extra stack.
    "_mb_audio_call_at:",
    "  stp x29, x30, [sp, #-32]!",
    "  stp x19, x20, [sp, #16]",
    "  mov x29, sp",
    "  mov x19, sp",
    "  sub x9, sp, #2048",
    "  mov x10, #-1024",
    "  and x9, x9, x10",
    "  add x9, x9, x7",
    "  mov sp, x9",
    "  blr x6",
    "  mov sp, x19",
    "  ldp x19, x20, [sp, #16]",
    "  ldp x29, x30, [sp], #32",
    "  ret",
);

#[cfg(target_arch = "aarch64")]
unsafe extern "C" {
    fn mb_audio_call_at(a0: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize, f: usize, sp: usize) -> usize;
}

/// `AudioConverterFillComplexBuffer(conv, input_proc, user, io, list, NULL)` entered at
/// [`fill_sp`].
unsafe fn fill_complex_buffer(conv: AudioConverterRef, user: *mut c_void, io: *mut u32, list: *mut AudioBufferList) -> OSStatus {
    #[cfg(target_arch = "aarch64")]
    // SAFETY: the trampoline passes the six arguments unchanged and restores the stack pointer.
    unsafe {
        mb_audio_call_at(
            conv as usize,
            input_proc as InputProc as usize,
            user as usize,
            io as usize,
            list as usize,
            0,
            AudioConverterFillComplexBuffer as *const () as usize,
            fill_sp(),
        ) as u32 as OSStatus
    }
    #[cfg(not(target_arch = "aarch64"))]
    // SAFETY: as documented on the caller.
    unsafe {
        AudioConverterFillComplexBuffer(conv, input_proc, user, io, list, std::ptr::null_mut())
    }
}

/// The AAC frame decoder (`FileReadMp4::decode`).
pub(crate) struct AacDecoder {
    conv: Converter,
    frames: Vec<FrameInfo>,
    info: ReaderInfo,
    next_frame: i32,
    state: DecodeState,
    bytes: Vec<u8>,
}

impl AacDecoder {
    pub fn open(path: &Path, track: &Mp4Track) -> Result<Self> {
        if track.codec != Codec::Aac {
            return Err(Error::Format("not AAC".into()));
        }
        if track.sample_size != 16 {
            return Err(Error::Format("AAC sample entry is not 16-bit".into()));
        }
        let mut frames = track.frames.clone();
        let mut rate = track.asc_rate.unwrap_or(track.entry_rate);
        let mut total = track.total;
        if track.sbr {
            // getFrameMap doubles the stts deltas for SBR streams.
            let mut pos = 0i32;
            for f in &mut frames {
                f.samples <<= 1;
                f.pos = pos;
                pos += f.samples as i32;
            }
            total <<= 1;
        }
        let mut conv = Converter::open(path, Vec::new(), None, Some(16), true)?;
        let fid = conv.src.format_id;
        let aac_ids = [b"aac ", b"aach", b"aacl", b"aacp", b"aacs"].map(fourcc);
        if !aac_ids.contains(&fid) {
            return Err(Error::Format("AudioFile data format is not AAC".into()));
        }
        // decoderOpen: when the file reports half the parsed rate (HE-AAC core rate), halve the
        // rate, the length and the frame map.
        if conv.src.sample_rate * 2.0 == f64::from(rate) {
            rate /= 2;
            total /= 2;
            for f in &mut frames {
                f.pos /= 2;
                f.samples /= 2;
            }
        }
        // The converter must output at `rate`: rebuild it with that destination rate.
        conv = Converter::open(path, frames.iter().map(|f| f.samples).collect(), Some(f64::from(rate)), Some(16), true)?;
        let info = ReaderInfo { sample_rate: rate, channels: conv.channels as u16, bits: 16, length: total as u32 };
        Ok(Self { conv, frames, info, next_frame: -1, state: DecodeState::new(), bytes: Vec::new() })
    }
}

impl FrameDecoder for AacDecoder {
    fn frames(&self) -> &[FrameInfo] {
        &self.frames
    }
    fn info(&self) -> ReaderInfo {
        self.info
    }
    fn scale(&self) -> f32 {
        1.0 / 32768.0
    }
    fn next_frame(&mut self) -> &mut i32 {
        &mut self.next_frame
    }
    fn state(&mut self) -> &mut DecodeState {
        &mut self.state
    }

    fn decode(&mut self, pos: i32, first: i32, reset: bool) -> bool {
        self.state.index = 0;
        self.state.len = 0;
        if reset {
            self.conv.reset(i64::from(first));
        }
        let ch = self.conv.channels as i32;
        let mut i = first.max(0);
        while (i as usize) < self.frames.len() {
            let f = self.frames[i as usize];
            let mut want = f.samples;
            if want < 2 {
                want = self.conv.src.frames_per_packet;
            }
            let mut bytes = std::mem::take(&mut self.bytes);
            let (st, got, nbytes) = self.conv.fill(want, &mut bytes);
            if st != 0 || got == 0 {
                self.bytes = bytes;
                break;
            }
            if self.next_frame <= i {
                self.next_frame += 1;
                self.state.index = (pos - f.pos) * ch;
                let n = (nbytes / 2) as usize;
                self.state.pcm.clear();
                self.state.pcm.extend(bytes[..n * 2].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
                self.state.len = n as i32;
                self.bytes = bytes;
                break;
            }
            self.bytes = bytes;
            i += 1;
        }
        self.state.len != 0
    }
}

/// ALAC: packets decoded one at a time, placed at their `stts` positions.
pub struct AlacReader {
    conv: Converter,
    frames: Vec<FrameInfo>,
    info: ReaderInfo,
    /// Decoded packet currently held and its index.
    packet: i64,
    pcm: Vec<Float2>,
    bytes: Vec<u8>,
}

impl AlacReader {
    pub fn open(path: &Path, track: &Mp4Track) -> Result<Self> {
        if track.codec != Codec::Alac {
            return Err(Error::Format("not ALAC".into()));
        }
        let bits = u32::from(track.alac_bits.unwrap_or(track.sample_size as u8));
        // FileReadAlac::analyze: 16 or 24 bits, at most two channels.
        if (bits | 8) != 24 || track.channels > 2 {
            return Err(Error::Format(format!("ALAC {bits}-bit")));
        }
        let mut conv = Converter::open(path, track.frames.iter().map(|f| f.samples).collect(), None, Some(bits), false)?;
        conv.feed.whole_buffer = true;
        let rate = track.alac_rate.filter(|&r| r != 0).unwrap_or(conv.src.sample_rate as u32);
        let info = ReaderInfo { sample_rate: rate, channels: conv.channels as u16, bits: bits as u16, length: track.total as u32 };
        Ok(Self { conv, frames: track.frames.clone(), info, packet: -1, pcm: Vec::new(), bytes: Vec::new() })
    }

    /// Decodes packet `i` into `pcm`, resetting the converter unless it follows the last one.
    fn load(&mut self, i: i64) -> bool {
        if i != self.packet + 1 || self.packet < 0 {
            self.conv.reset(i);
        }
        let want = self.frames[i as usize].samples;
        let mut bytes = std::mem::take(&mut self.bytes);
        let (st, got, _) = self.conv.fill(want, &mut bytes);
        self.pcm.clear();
        if st == 0 {
            let bps = self.conv.bytes_per_sample as usize;
            let ch = self.conv.channels as usize;
            let conv = |b: &[u8]| -> f32 {
                // Little-endian packed integer in the top bits of an i32, · 2^-31.
                let mut v: u32 = 0;
                for k in (0..bps).rev() {
                    v = (v << 8) | u32::from(b[k]);
                }
                ((v << (32 - 8 * bps)) as i32) as f32 * (1.0 / 2_147_483_648.0)
            };
            for fr in bytes[..got as usize * bps * ch].chunks_exact(bps * ch) {
                self.pcm.push([conv(&fr[..bps]), conv(&fr[(ch - 1) * bps..ch * bps])]);
            }
        }
        self.bytes = bytes;
        self.packet = i;
        st == 0
    }
}

impl Reader for AlacReader {
    fn info(&self) -> ReaderInfo {
        self.info
    }

    fn read(&mut self, pos: i64, out: &mut [Float2]) {
        out.fill([0.0; 2]);
        let total = i64::from(self.info.length);
        let mut p = pos.max(0);
        let end = (pos + out.len() as i64).min(total);
        while p < end {
            // Packet containing p.
            let i = match self.frames.binary_search_by(|f| i64::from(f.pos).cmp(&p)) {
                Ok(i) => i,
                Err(0) => return,
                Err(i) => i - 1,
            } as i64;
            if i != self.packet && !self.load(i) {
                return;
            }
            let start = i64::from(self.frames[i as usize].pos);
            let avail = start + self.pcm.len() as i64;
            if p >= avail {
                // Packet shorter than its stts delta: the rest stays silent.
                let next = self.frames.get(i as usize + 1).map_or(end, |f| i64::from(f.pos));
                p = next.max(p + 1);
                continue;
            }
            let n = (avail.min(end) - p) as usize;
            let src = (p - start) as usize;
            let dst = (p - pos) as usize;
            out[dst..dst + n].copy_from_slice(&self.pcm[src..src + n]);
            p += n as i64;
        }
    }
}
