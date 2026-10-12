//! Preview playback on the mb-audio decode spine.
//!
//! [`Player`] decodes on a background thread with [`mb_audio::Decoder`] (rekordbox's timeline:
//! cue and grid positions in ms line up with what rekordbox shows) and plays through the default
//! output device with cpal. The device keeps its own rate; the engine converts with linear
//! interpolation (preview quality — analysis never goes through the player).
//!
//! The UI reads the playhead lock-free through a [`Clock`].

mod engine;

pub use engine::{CHUNK, Clock, Engine, Renderer};
pub use mb_audio::StreamInfo;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Decode(#[from] mb_audio::Error),
    #[error("audio output: {0}")]
    Output(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A file loaded for preview, attached to the default output device.
pub struct Player {
    engine: Engine,
    _stream: cpal::Stream,
    device_rate: u32,
}

impl Player {
    /// Opens `path` and connects to the default output device (paused).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let decoder = mb_audio::open(path)?;
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| Error::Output("no output device".into()))?;
        let supported = device.default_output_config().map_err(|e| Error::Output(e.to_string()))?;
        let config = supported.config();
        let (engine, renderer) = Engine::new(decoder, config.sample_rate);
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, renderer),
            cpal::SampleFormat::I16 => build::<i16>(&device, config, renderer),
            cpal::SampleFormat::I32 => build::<i32>(&device, config, renderer),
            cpal::SampleFormat::U16 => build::<u16>(&device, config, renderer),
            other => return Err(Error::Output(format!("unsupported sample format {other:?}"))),
        }?;
        stream.play().map_err(|e| Error::Output(e.to_string()))?;
        Ok(Self { engine, _stream: stream, device_rate: config.sample_rate })
    }

    pub fn info(&self) -> &StreamInfo {
        self.engine.info()
    }

    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// Lock-free playhead for the UI.
    pub fn clock(&self) -> Clock {
        self.engine.clock()
    }

    pub fn position_ms(&self) -> f64 {
        self.engine.clock().position_ms()
    }

    pub fn is_playing(&self) -> bool {
        self.engine.clock().is_playing()
    }

    pub fn play(&self) {
        self.engine.play();
    }

    pub fn pause(&self) {
        self.engine.pause();
    }

    pub fn toggle(&self) {
        if self.is_playing() { self.pause() } else { self.play() }
    }

    pub fn seek_ms(&self, ms: f64) {
        self.engine.seek_ms(ms);
    }

    pub fn seek_sample(&self, sample: u64) {
        self.engine.seek_sample(sample);
    }

    /// Starts playback exactly at a cue position.
    pub fn play_from_ms(&self, ms: f64) {
        self.engine.play_from_ms(ms);
    }

    /// Linear output gain (1.0 = unchanged).
    pub fn set_volume(&self, gain: f32) {
        self.engine.set_volume(gain);
    }
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, mut renderer: Renderer) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let mut scratch: Vec<f32> = Vec::with_capacity(1 << 15);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let ts = info.timestamp();
                renderer.set_latency(ts.playback.duration_since(ts.callback));
                if scratch.len() < data.len() {
                    scratch.resize(data.len(), 0.0);
                }
                let buf = &mut scratch[..data.len()];
                renderer.render(buf, channels);
                for (d, s) in data.iter_mut().zip(buf.iter()) {
                    *d = T::from_sample(*s);
                }
            },
            |err| eprintln!("mb-player: stream error: {err}"),
            None,
        )
        .map_err(|e| Error::Output(e.to_string()))
}
