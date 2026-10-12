//! Device-independent playback engine: a decode thread feeding a realtime-safe ring buffer, and a
//! render function for the audio callback. The audio side never blocks, allocates or locks.

use mb_audio::{Decoder, Float2, StreamInfo};
use rtrb::{Consumer, Producer, RingBuffer};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// Samples per ring-buffer chunk.
pub const CHUNK: usize = 1024;
/// Chunks the decode thread keeps ahead (≈ 0.75 s at 44.1 kHz).
const RING_CHUNKS: usize = 32;

struct Chunk {
    epoch: u32,
    /// Track position of `data[0]`.
    start: u64,
    len: usize,
    data: [Float2; CHUNK],
}

/// State shared between the control side, the decode thread and the audio callback.
pub struct Shared {
    playing: AtomicBool,
    volume: AtomicU32,
    /// Incremented by every seek; chunks of older epochs are dropped.
    epoch: AtomicU32,
    seek_target: AtomicU64,
    /// Next track sample handed to the device (the playback clock).
    position: AtomicU64,
    /// Device output latency estimate in nanoseconds (0 when unknown).
    latency_ns: AtomicU64,
    finished: AtomicBool,
    shutdown: AtomicBool,
    pub info: StreamInfo,
}

/// A cheap, lock-free read handle on the playback clock for the UI thread.
#[derive(Clone)]
pub struct Clock(pub(crate) Arc<Shared>);

impl Clock {
    /// Next sample (at the decoder's rate) the device receives.
    pub fn position_samples(&self) -> u64 {
        self.0.position.load(Ordering::Acquire)
    }

    /// [`Clock::position_samples`] in milliseconds on rekordbox's timeline.
    pub fn position_ms(&self) -> f64 {
        mb_audio::timeline::sample_to_ms(self.position_samples() as i64, self.0.info.sample_rate)
    }

    /// Estimated time between handing a sample to the device and hearing it.
    pub fn latency_ms(&self) -> f64 {
        self.0.latency_ns.load(Ordering::Relaxed) as f64 / 1e6
    }

    pub fn is_playing(&self) -> bool {
        self.0.playing.load(Ordering::Acquire)
    }

    /// True once playback ran past the end of the track.
    pub fn finished(&self) -> bool {
        self.0.finished.load(Ordering::Acquire)
    }

    pub fn duration_ms(&self) -> f64 {
        mb_audio::timeline::sample_to_ms(self.0.info.total_samples as i64, self.0.info.sample_rate)
    }
}

/// Owns the decode thread; control methods are callable from any thread.
pub struct Engine {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// Audio-callback side of the engine.
pub struct Renderer {
    shared: Arc<Shared>,
    consumer: Consumer<Chunk>,
    epoch: u32,
    /// Samples popped from the ring but not yet played, starting at `local_start`.
    local: VecDeque<Float2>,
    local_start: u64,
    /// Fractional read position into `local` (linear interpolation for rate conversion).
    frac: f64,
    /// Track rate / device rate.
    ratio: f64,
}

impl Engine {
    /// Starts the decode thread. `device_rate` is the output rate the renderer will be driven at.
    pub fn new(decoder: Decoder, device_rate: u32) -> (Self, Renderer) {
        let info = *decoder.info();
        let shared = Arc::new(Shared {
            playing: AtomicBool::new(false),
            volume: AtomicU32::new(1.0f32.to_bits()),
            epoch: AtomicU32::new(0),
            seek_target: AtomicU64::new(0),
            position: AtomicU64::new(0),
            latency_ns: AtomicU64::new(0),
            finished: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            info,
        });
        let (producer, consumer) = RingBuffer::<Chunk>::new(RING_CHUNKS);
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("mb-player-decode".into())
            .spawn(move || decode_loop(decoder, producer, s))
            .expect("spawn decode thread");
        let renderer = Renderer {
            shared: shared.clone(),
            consumer,
            epoch: 0,
            local: VecDeque::with_capacity(CHUNK * 4),
            local_start: 0,
            frac: 0.0,
            ratio: f64::from(info.sample_rate) / f64::from(device_rate.max(1)),
        };
        (Self { shared, thread: Some(thread) }, renderer)
    }

    pub fn clock(&self) -> Clock {
        Clock(self.shared.clone())
    }

    pub fn info(&self) -> &StreamInfo {
        &self.shared.info
    }

    pub fn play(&self) {
        self.shared.playing.store(true, Ordering::Release);
    }

    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Release);
    }

    pub fn set_volume(&self, gain: f32) {
        self.shared.volume.store(gain.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// Moves the playhead to `sample`; playback (if running) continues from there. The clock
    /// jumps immediately.
    pub fn seek_sample(&self, sample: u64) {
        let s = sample.min(self.shared.info.total_samples);
        self.shared.seek_target.store(s, Ordering::Release);
        self.shared.position.store(s, Ordering::Release);
        self.shared.finished.store(false, Ordering::Release);
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
    }

    pub fn seek_ms(&self, ms: f64) {
        self.seek_sample(mb_audio::timeline::ms_to_sample(ms, self.shared.info.sample_rate).max(0) as u64);
    }

    /// Starts playback exactly at `sample`: the first sample sent to the device is that sample
    /// (the renderer outputs silence, without advancing the clock, until it is decoded).
    pub fn play_from_sample(&self, sample: u64) {
        self.seek_sample(sample);
        self.play();
    }

    pub fn play_from_ms(&self, ms: f64) {
        self.seek_ms(ms);
        self.play();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn decode_loop(mut decoder: Decoder, mut producer: Producer<Chunk>, shared: Arc<Shared>) {
    let total = shared.info.total_samples;
    let mut epoch = u32::MAX;
    while !shared.shutdown.load(Ordering::Acquire) {
        let e = shared.epoch.load(Ordering::Acquire);
        if e != epoch {
            epoch = e;
            decoder.seek(shared.seek_target.load(Ordering::Acquire));
        }
        if producer.slots() == 0 || decoder.position() >= total {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        let start = decoder.position();
        let mut chunk = Chunk { epoch: e, start, len: 0, data: [[0.0; 2]; CHUNK] };
        chunk.len = decoder.read_frames(&mut chunk.data);
        if shared.epoch.load(Ordering::Acquire) != e {
            continue;
        }
        let _ = producer.push(chunk);
    }
}

impl Renderer {
    /// Driven by the device callback: fills interleaved `out` with `channels` channels.
    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        let shared = self.shared.clone();
        let sh = &*shared;
        let e = sh.epoch.load(Ordering::Acquire);
        if e != self.epoch {
            self.epoch = e;
            self.local.clear();
            self.frac = 0.0;
            self.local_start = sh.seek_target.load(Ordering::Acquire);
        }
        if !sh.playing.load(Ordering::Acquire) {
            out.fill(0.0);
            return;
        }
        let gain = f32::from_bits(sh.volume.load(Ordering::Relaxed));
        let total = sh.info.total_samples;
        for frame in out.chunks_mut(channels) {
            let i0 = self.frac as usize;
            if !self.fill(i0 + 2, total) {
                // Underrun: silence without advancing the clock.
                frame.fill(0.0);
                continue;
            }
            if self.local_start + i0 as u64 >= total {
                frame.fill(0.0);
                sh.finished.store(true, Ordering::Release);
                sh.playing.store(false, Ordering::Release);
                continue;
            }
            let t = (self.frac - i0 as f64) as f32;
            let a = self.local[i0];
            let b = self.local[i0 + 1];
            let s = [(a[0] + (b[0] - a[0]) * t) * gain, (a[1] + (b[1] - a[1]) * t) * gain];
            match channels {
                1 => frame[0] = 0.5 * (s[0] + s[1]),
                _ => {
                    frame[0] = s[0];
                    frame[1] = s[1];
                    frame[2..].fill(0.0);
                }
            }
            self.frac += self.ratio;
            let k = self.frac as usize;
            if k > 0 {
                let k = k.min(self.local.len());
                self.local.drain(..k);
                self.local_start += k as u64;
                self.frac -= k as f64;
            }
        }
        sh.position.store(self.local_start + self.frac as u64, Ordering::Release);
    }

    /// Makes `need` samples available in `local` (zero-padding past the end of the track).
    fn fill(&mut self, need: usize, total: u64) -> bool {
        while self.local.len() < need {
            let end = self.local_start + self.local.len() as u64;
            if end >= total {
                self.local.push_back([0.0; 2]);
                continue;
            }
            match self.consumer.pop() {
                Ok(c) if c.epoch == self.epoch && c.start == end => self.local.extend(&c.data[..c.len]),
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        true
    }

    /// Records the device latency reported by the backend.
    pub fn set_latency(&self, latency: Duration) {
        self.shared.latency_ns.store(latency.as_nanos() as u64, Ordering::Relaxed);
    }
}
