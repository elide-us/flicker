//! Live output through the default device (cpal), behind the `playback`
//! feature. The compiled graph is moved into the stream callback and processed
//! in blocks; the callback does no musical arithmetic and no file work.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use fundsp::prelude32::{AudioUnit, BufferRef, BufferVec, MAX_BUFFER_SIZE};

use crate::score::Score;
use crate::synth;

#[derive(Debug, thiserror::Error)]
pub enum PlayError {
    #[error("no default output device")]
    NoDevice,
    #[error(transparent)]
    Device(#[from] cpal::Error),
    #[error("device sample format {0:?} is not supported")]
    Format(cpal::SampleFormat),
}

/// What the device gave us and what was scheduled.
pub struct PlayReport {
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_secs: f64,
    pub warnings: Vec<String>,
    pub voices: usize,
}

/// A running stream. Drop it to stop; [`Playing::wait`] blocks until the piece ends.
pub struct Playing {
    _stream: cpal::Stream,
    pub report: PlayReport,
    underruns: Arc<AtomicU32>,
}

impl Playing {
    /// Buffer underruns the device has reported so far. Anything above zero
    /// means the callback missed a deadline and the listener heard a gap.
    pub fn underruns(&self) -> u32 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Block until the piece (and its tail) has played, then stop. Returns the
    /// underrun count for the whole run.
    pub fn wait(self) -> u32 {
        std::thread::sleep(Duration::from_secs_f64(self.report.duration_secs + 0.25));
        self.underruns()
    }
}

/// Compile `score` for the default device and start playing it.
pub fn start(score: &Score) -> Result<Playing, PlayError> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(PlayError::NoDevice)?;
    let supported = device.default_output_config()?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    match format {
        cpal::SampleFormat::F32 => run::<f32>(score, &device, &config),
        cpal::SampleFormat::I16 => run::<i16>(score, &device, &config),
        cpal::SampleFormat::U16 => run::<u16>(score, &device, &config),
        other => Err(PlayError::Format(other)),
    }
}

fn run<T: SizedSample + FromSample<f32>>(
    score: &Score,
    device: &cpal::Device,
    config: &cpal::StreamConfig,
) -> Result<Playing, PlayError> {
    let sample_rate = config.sample_rate;
    let channels = config.channels as usize;
    let built = synth::build(score, sample_rate as f64);
    let mut master = built.master;
    master.allocate();
    let mut buffer = BufferVec::new(2);
    let underruns = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&underruns);

    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            for block in data.chunks_mut(channels * MAX_BUFFER_SIZE) {
                let frames = block.len() / channels;
                let mut out = buffer.buffer_mut();
                master.process(frames, &BufferRef::new(&[]), &mut out);
                for (i, frame) in block.chunks_mut(channels).enumerate() {
                    let (l, r) = (out.at_f32(0, i), out.at_f32(1, i));
                    if channels == 1 {
                        frame[0] = T::from_sample((l + r) * 0.5);
                    } else {
                        frame[0] = T::from_sample(l);
                        frame[1] = T::from_sample(r);
                        for extra in &mut frame[2..] {
                            *extra = T::from_sample(0.0);
                        }
                    }
                }
            }
        },
        move |err| {
            if matches!(err.kind(), cpal::ErrorKind::Xrun) {
                counter.fetch_add(1, Ordering::Relaxed);
            }
            tracing::error!("audio stream error: {err}");
        },
        None,
    )?;
    stream.play()?;
    Ok(Playing {
        _stream: stream,
        underruns,
        report: PlayReport {
            sample_rate,
            channels: config.channels,
            duration_secs: built.duration_secs,
            warnings: built.warnings,
            voices: built.voices,
        },
    })
}
